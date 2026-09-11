use crate::{executor::block_on, host::NativeHost};
use bexos_migration::Error;
use bexos_userspace::{
    Channel,
    live_migration::{self, Resource, State},
};
use bexos_wasm_runtime::{
    budget::Budget, context::Context, migration::Snapshot, service_guest::ServiceGuest,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
mod codec;
mod grant_codec;
#[cfg(test)]
mod tests;
mod wasi_codec;
const CHUNK: usize = 30_000;
const MAX_RUNTIME_STATE: usize = codec::MAX_STATE + (5 << 20);
pub struct Runtime {
    pub instance: Option<ServiceGuest>,
    pub control: Channel,
    pub migration: Option<Channel>,
    records: BTreeMap<u64, Vec<u8>>,
    snapshot: Option<Snapshot>,
    candidate: Option<(
        wasmtime::Engine,
        Arc<[u8]>,
        bexos_wasm_abi::WasmRunnerOptions,
        Arc<NativeHost>,
    )>,
    prepared: Option<Arc<Vec<bexos_wasm_runtime::prepared::PreparedService>>>,
    host: Option<Arc<NativeHost>>,
    ownership: Arc<AtomicBool>,
    valid: bool,
    current: bool,
}
impl Runtime {
    pub fn source(
        instance: ServiceGuest,
        control: Channel,
        migration: Option<Channel>,
        host: Arc<NativeHost>,
    ) -> Self {
        Self {
            instance: Some(instance),
            control,
            migration,
            host: Some(host),
            ..Self::empty()
        }
    }
    pub fn candidate(
        engine: wasmtime::Engine,
        bytes: Arc<[u8]>,
        options: bexos_wasm_abi::WasmRunnerOptions,
        host: Arc<NativeHost>,
        migration: Channel,
    ) -> Self {
        Self {
            candidate: Some((engine, bytes, options, host)),
            migration: Some(migration),
            ..Self::empty()
        }
    }
    pub fn refresh(&mut self) -> wasmtime::Result<()> {
        // A busy renderer or a failed refresh must not invalidate the previous
        // bulk snapshot. Only a successful refresh makes cutover current again.
        self.current = false;
        if self.host.as_ref().is_some_and(|host| !host.ui_quiescent()) {
            return Err(wasmtime::format_err!("native UI renderer busy"));
        }
        let snapshot = block_on(
            self.instance
                .as_mut()
                .ok_or_else(|| wasmtime::format_err!("missing instance"))?
                .snapshot(),
        )?;
        let mut encoded = codec::encode(&snapshot)
            .map_err(|e| wasmtime::format_err!("checkpoint codec: {e:?}"))?;
        let ui = self
            .host
            .as_ref()
            .ok_or_else(|| wasmtime::format_err!("missing native host"))?
            .ui
            .lock()
            .unwrap()
            .encode()
            .map_err(|e| wasmtime::format_err!("UI checkpoint: {e:?}"))?;
        let ui_len = ui.len();
        encoded.extend(ui);
        if encoded.len() > MAX_RUNTIME_STATE {
            return Err(wasmtime::format_err!("runtime checkpoint capacity"));
        }
        self.records.clear();
        let mut meta = Vec::new();
        meta.extend_from_slice(&self.control.0.to_le_bytes());
        meta.extend_from_slice(&(encoded.len() as u64).to_le_bytes());
        meta.extend_from_slice(&self.migration.map_or(0, |c| c.0).to_le_bytes());
        meta.extend_from_slice(&(ui_len as u64).to_le_bytes());
        self.records.insert(0, meta);
        for (i, chunk) in encoded.chunks(CHUNK).enumerate() {
            self.records.insert(i as u64 + 1, chunk.to_vec());
        }
        self.snapshot = Some(snapshot);
        self.valid = true;
        self.current = true;
        Ok(())
    }
    pub fn invalidate(&mut self) {
        // Bulk copying may use the last safe snapshot while dispatch runs.
        // Cutover must wait for a fresh snapshot of the current instance.
        self.current = false;
    }
    pub fn discard_checkpoint(&mut self) {
        self.invalidate();
        self.valid = false;
        self.records.clear();
        self.snapshot = None;
    }
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            instance: None,
            control: Channel(0),
            migration: None,
            records: BTreeMap::new(),
            snapshot: None,
            candidate: None,
            prepared: None,
            host: None,
            ownership: Arc::new(AtomicBool::new(false)),
            valid: false,
            current: false,
        }
    }
    fn migration_state_limit() -> usize {
        MAX_RUNTIME_STATE + 32
    }
    fn keys(&self) -> Vec<u64> {
        self.records.keys().copied().collect()
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if !self.valid {
            return Err(Error::BadState);
        }
        Ok(self.records.get(&key).cloned())
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key > MAX_RUNTIME_STATE.div_ceil(CHUNK) as u64 {
            return Err(Error::Capacity);
        }
        if let Some(bytes) = bytes {
            if bytes.len() > CHUNK {
                return Err(Error::Capacity);
            }
            self.records.insert(key, bytes.to_vec());
        } else {
            self.records.remove(&key);
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        if self.valid
            && self.current
            && self.control.0 != 0
            && self.instance.is_some()
            && self.host.as_ref().is_none_or(|host| host.ui_quiescent())
        {
            Ok(())
        } else {
            Err(Error::BadState)
        }
    }
    fn quiescence_ready(&self) -> bool {
        // Fuel yields can service bulk requests using the last checkpoint.
        // Cutover must wait for dispatch to return and refresh that checkpoint;
        // a temporarily busy guest is not a failed replacement.
        self.current
            && self.instance.is_some()
            && self.host.as_ref().is_none_or(|host| host.ui_quiescent())
    }
    fn prepare_adoption(&mut self) -> Result<(), Error> {
        use bexos_wasm_runtime::prepared::PreparedService;
        let metadata = self
            .records
            .get(&0)
            .filter(|v| v.len() == 24 || v.len() == 32)
            .ok_or(Error::InvalidData)?;
        let length = usize::try_from(u64::from_le_bytes(metadata[8..16].try_into().unwrap()))
            .map_err(|_| Error::Capacity)?;
        if length > MAX_RUNTIME_STATE || self.records.len() != length.div_ceil(CHUNK) + 1 {
            return Err(Error::InvalidData);
        }
        let mut bytes = Vec::with_capacity(length);
        for index in 1..self.records.len() {
            bytes.extend_from_slice(
                self.records
                    .get(&(index as u64))
                    .ok_or(Error::InvalidData)?,
            );
        }
        if bytes.len() != length {
            return Err(Error::InvalidData);
        }
        let ui_len = if metadata.len() == 32 {
            usize::try_from(u64::from_le_bytes(metadata[24..32].try_into().unwrap()))
                .map_err(|_| Error::Capacity)?
        } else {
            0
        };
        let split = bytes.len().checked_sub(ui_len).ok_or(Error::InvalidData)?;
        let snapshot = codec::decode(&bytes[..split], self.ownership.clone())?;
        let (engine, replacement, options, _) = self.candidate.as_ref().ok_or(Error::BadState)?;
        if &snapshot.options != options {
            return Err(Error::InvalidData);
        }
        fn compile_tree(
            snapshot: &Snapshot,
            engine: &wasmtime::Engine,
            replacement: Option<Arc<[u8]>>,
            output: &mut Vec<PreparedService>,
        ) -> wasmtime::Result<()> {
            let bytes = replacement.unwrap_or_else(|| snapshot.bytes.clone());
            if !output.iter().any(|compiled| compiled.matches(&bytes)) {
                output.push(PreparedService::compile(
                    engine,
                    bytes,
                    snapshot.options.limits.max_module_bytes,
                )?);
            }
            for (_, child) in &snapshot.children {
                compile_tree(child, engine, None, output)?;
            }
            Ok(())
        }
        let mut prepared = Vec::new();
        compile_tree(&snapshot, engine, Some(replacement.clone()), &mut prepared).map_err(
            |error| {
                bexos_userspace::log(&format!("wasm_runner: preparation rejected: {error:#}\n"));
                Error::InvalidData
            },
        )?;
        let prepared = Arc::new(prepared);
        let host = self.candidate.as_ref().unwrap().3.clone();
        host.replace_grants(snapshot.resources.iter().map(|(_, entry)| entry));
        let context = Context::new(
            options.clone(),
            host,
            snapshot.origin,
            Budget::for_limits(&options.limits),
        );
        self.instance = Some(
            block_on(snapshot.prepare_candidate(
                engine.clone(),
                context,
                Some(replacement.clone()),
                prepared.clone(),
            ))
            .map_err(|error| {
                bexos_userspace::log(&format!(
                    "wasm_runner: candidate allocation rejected: {error:#}\n"
                ));
                Error::InvalidData
            })?,
        );
        self.prepared = Some(prepared);
        bexos_userspace::log("wasm_runner: candidate compilation and stores prepared\n");
        Ok(())
    }
    fn finish_adoption(&mut self) -> Result<(), Error> {
        bexos_userspace::log("wasm_runner: restoring candidate checkpoint\n");
        let metadata = self
            .records
            .get(&0)
            .filter(|v| v.len() == 24 || v.len() == 32)
            .ok_or(Error::InvalidData)?;
        self.control = Channel(u64::from_le_bytes(metadata[..8].try_into().unwrap()));
        let length = usize::try_from(u64::from_le_bytes(metadata[8..16].try_into().unwrap()))
            .map_err(|_| Error::Capacity)?;
        let migration = u64::from_le_bytes(metadata[16..24].try_into().unwrap());
        self.migration = if migration == 0 {
            None
        } else {
            Some(Channel(migration))
        };
        if length > MAX_RUNTIME_STATE || self.records.len() != length.div_ceil(CHUNK) + 1 {
            return Err(Error::InvalidData);
        }
        let mut bytes = Vec::with_capacity(length);
        for i in 1..self.records.len() {
            bytes.extend_from_slice(self.records.get(&(i as u64)).ok_or(Error::InvalidData)?);
        }
        if bytes.len() != length {
            return Err(Error::InvalidData);
        }
        let ui_len = if metadata.len() == 32 {
            usize::try_from(u64::from_le_bytes(metadata[24..32].try_into().unwrap()))
                .map_err(|_| Error::Capacity)?
        } else {
            0
        };
        let split = bytes.len().checked_sub(ui_len).ok_or(Error::InvalidData)?;
        let snapshot = codec::decode(&bytes[..split], self.ownership.clone())?;
        bexos_userspace::log("wasm_runner: checkpoint decoded\n");
        let (_, _, _, host) = self.candidate.take().ok_or(Error::BadState)?;
        host.replace_grants(snapshot.resources.iter().map(|(_, entry)| entry));
        *host.ui.lock().unwrap() =
            crate::host::ui::UiState::decode(&bytes[split..], &snapshot.resources)?;
        self.host = Some(host.clone());
        self.prepared.take().ok_or(Error::BadState)?;
        self.snapshot = Some(snapshot.clone());
        block_on(snapshot.install_checkpoint(self.instance.as_mut().ok_or(Error::BadState)?, true))
            .map_err(|error| {
                bexos_userspace::log(&format!("wasm_runner: restore rejected: {error:#}\n"));
                Error::InvalidData
            })?;
        bexos_userspace::log("wasm_runner: candidate restore complete\n");
        self.valid = true;
        self.current = true;
        self.validate()
    }
    fn resources(&self) -> Vec<Resource> {
        fn visit(snapshot: &Snapshot, out: &mut std::collections::BTreeSet<u64>) {
            for (_, entry) in &snapshot.resources {
                out.insert(entry.handle.native());
                out.extend(entry.handle.companions());
            }
            super::migration::wasi_codec::handles(&snapshot.wasi, out);
            for (_, child) in &snapshot.children {
                visit(child, out);
            }
        }
        let mut handles = std::collections::BTreeSet::new();
        handles.insert(self.control.0);
        if let Some(host) = &self.host {
            handles.extend(host.ui.lock().unwrap().resources());
        }
        if let Some(snapshot) = &self.snapshot {
            visit(snapshot, &mut handles);
        }
        handles.into_iter().map(Resource::Handle).collect()
    }
    fn activated(&mut self, _generation: u64) {
        self.ownership.store(true, Ordering::Release);
    }
}
pub fn receive(
    engine: wasmtime::Engine,
    bytes: Arc<[u8]>,
    options: bexos_wasm_abi::WasmRunnerOptions,
    host: Arc<NativeHost>,
    migration: Channel,
    generation: u64,
) -> wasmtime::Result<Runtime> {
    live_migration::receive_with_state(
        migration,
        generation,
        Runtime::candidate(engine, bytes, options, host, migration),
    )
    .map_err(|e| wasmtime::format_err!("migration reception: {e:?}"))
}
