use crate::host::NativeHandle;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_wasm_runtime::{
    migration::Snapshot,
    resources::{Entry, Handle, Kind, Origin},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};
pub const MAX_STATE: usize = 8 << 20;
pub fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, Error> {
    fn estimate(s: &Snapshot, remaining: &mut usize, depth: usize) -> Result<(), Error> {
        if depth > 64 {
            return Err(Error::Capacity);
        }
        let mut n = 512usize
            .checked_add(s.bytes.len())
            .and_then(|n| n.checked_add(s.checkpoint.len()))
            .ok_or(Error::Capacity)?;
        n = n
            .checked_add(s.options.encode().map_err(|_| Error::InvalidData)?.len())
            .ok_or(Error::Capacity)?;
        for dependency in &s.component_dependencies {
            n = n
                .checked_add(
                    dependency
                        .dependency
                        .import
                        .package_name
                        .len()
                        .saturating_add(dependency.dependency.import.export_name.len())
                        .saturating_add(dependency.dependency.import.instance_name.len())
                        .saturating_add(dependency.bytes.len())
                        .saturating_add(512),
                )
                .ok_or(Error::Capacity)?;
        }
        for (_, e) in &s.resources {
            n = n
                .checked_add(
                    e.name.len()
                        + 512
                        + e.handle.allowed_methods().map_or(0, |m| m.len() * 8)
                        + super::grant_codec::estimate(e.handle.grant()),
                )
                .ok_or(Error::Capacity)?;
        }
        n = n
            .checked_add(super::wasi_codec::estimate(&s.wasi)?)
            .ok_or(Error::Capacity)?;
        for m in &s.messages {
            n = n.checked_add(m.len() + 8).ok_or(Error::Capacity)?;
        }
        *remaining = remaining.checked_sub(n).ok_or(Error::Capacity)?;
        for (_, child) in &s.children {
            estimate(child, remaining, depth + 1)?;
        }
        Ok(())
    }
    let mut remaining = MAX_STATE;
    estimate(snapshot, &mut remaining, 0)?;
    let mut e = Encoder::new();
    e.word(4);
    write(&mut e, snapshot, 0)?;
    let bytes = e.finish();
    if bytes.len() > MAX_STATE {
        return Err(Error::Capacity);
    }
    Ok(bytes)
}
fn write(e: &mut Encoder, s: &Snapshot, depth: usize) -> Result<(), Error> {
    if depth > 64 {
        return Err(Error::Capacity);
    }
    e.bytes(&s.options.encode().map_err(|_| Error::InvalidData)?);
    e.word((s.origin == Origin::Signed) as u64);
    e.bytes(&s.bytes);
    e.bytes(&s.checkpoint);
    e.word(s.remaining_fuel);
    e.word(s.paused as u64);
    e.word(s.insecure_seed.is_some() as u64);
    if let Some((a, b)) = s.insecure_seed {
        e.word(a);
        e.word(b);
    }
    e.word(s.component_dependencies.len() as u64);
    for dependency in &s.component_dependencies {
        e.text(&dependency.dependency.import.package_name);
        e.text(&dependency.dependency.import.export_name);
        e.word(dependency.dependency.import.abi_version as u64);
        e.text(&dependency.dependency.import.instance_name);
        e.word(dependency.dependency.module_len);
        e.bytes(&dependency.bytes);
    }
    e.word(s.next_resource as u64);
    e.word(s.resources.len() as u64);
    for (id, entry) in &s.resources {
        e.word(*id as u64);
        write_entry(e, entry);
    }
    super::wasi_codec::write_table(e, &s.wasi)?;
    e.word(s.messages.len() as u64);
    for m in &s.messages {
        e.bytes(m);
    }
    e.word(s.next_child as u64);
    e.word(s.children.len() as u64);
    for (id, child) in &s.children {
        e.word(*id as u64);
        write(e, child, depth + 1)?;
    }
    Ok(())
}
pub fn decode(bytes: &[u8], ownership: Arc<AtomicBool>) -> Result<Snapshot, Error> {
    if bytes.len() > MAX_STATE {
        return Err(Error::Capacity);
    }
    let mut d = Decoder::new(bytes);
    let version = d.word()?;
    if version != 2 && version != 3 && version != 4 {
        return Err(Error::InvalidData);
    }
    let s = read(&mut d, ownership, &mut BTreeMap::new(), &mut 0, 0, version)?;
    d.finish()?;
    Ok(s)
}
pub(super) fn word32(d: &mut Decoder<'_>) -> Result<u32, Error> {
    u32::try_from(d.word()?).map_err(|_| Error::InvalidData)
}
fn read(
    d: &mut Decoder<'_>,
    ownership: Arc<AtomicBool>,
    handles: &mut BTreeMap<u64, Arc<dyn Handle>>,
    count: &mut usize,
    depth: usize,
    version: u64,
) -> Result<Snapshot, Error> {
    *count += 1;
    if *count > 65 || depth > 64 {
        return Err(Error::Capacity);
    }
    let options = bexos_wasm_abi::WasmRunnerOptions::decode(d.bytes(32768)?)
        .map_err(|_| Error::InvalidData)?;
    let origin = if d.flag()? {
        Origin::Signed
    } else {
        Origin::Unsigned
    };
    let bytes = d.bytes(options.limits.max_module_bytes as usize)?.into();
    let checkpoint = d.bytes(1 << 20)?.to_vec();
    let remaining_fuel = d.word()?;
    if remaining_fuel > options.limits.fuel {
        return Err(Error::InvalidData);
    }
    let paused = d.flag()?;
    let insecure_seed = if version >= 3 && d.flag()? {
        Some((d.word()?, d.word()?))
    } else {
        None
    };
    let mut component_dependencies = Vec::new();
    if version >= 4 {
        for _ in 0..d.count(16)? {
            let package_name = d.text(128)?.to_string();
            let export_name = d.text(128)?.to_string();
            let abi_version = word32(d)?;
            let instance_name = d.text(128)?.to_string();
            let module_len = d.word()?;
            let bytes: Arc<[u8]> = d.bytes(options.limits.max_module_bytes as usize)?.into();
            if bytes.len() as u64 != module_len {
                return Err(Error::InvalidData);
            }
            component_dependencies.push(bexos_wasm_runtime::migration::ComponentPayload {
                dependency: bexos_wasm_abi::ComponentDependency {
                    import: bexos_wasm_abi::ComponentImport {
                        package_name,
                        export_name,
                        abi_version,
                        instance_name,
                    },
                    module_len,
                },
                bytes,
            });
        }
    }
    let next_resource = word32(d)?;
    let mut resources = Vec::new();
    for _ in 0..d.count(options.limits.max_handles as usize)? {
        let id = word32(d)?;
        let entry = read_entry(d, ownership.clone(), handles)?;
        resources.push((id, entry));
    }
    let wasi = super::wasi_codec::read_table(
        d,
        ownership.clone(),
        handles,
        options.limits.max_handles as usize,
    )?;
    let mut messages = std::collections::VecDeque::new();
    for _ in 0..d.count(16)? {
        messages.push_back(d.bytes(32768)?.to_vec());
    }
    let next_child = word32(d)?;
    let mut children = Vec::new();
    for _ in 0..d.count(options.limits.max_children as usize)? {
        let id = word32(d)?;
        children.push((
            id,
            read(d, ownership.clone(), handles, count, depth + 1, version)?,
        ));
    }
    Ok(Snapshot {
        insecure_seed,
        options,
        origin,
        bytes,
        checkpoint,
        wasi,
        resources,
        next_resource,
        children,
        next_child,
        messages,
        remaining_fuel,
        paused,
        component_dependencies,
    })
}

pub(super) fn write_entry(e: &mut Encoder, entry: &Entry) {
    e.text(&entry.name);
    e.word(entry.handle.native());
    e.word(match entry.handle.kind() {
        Kind::Channel => 1,
        Kind::Directory => 2,
        Kind::File => 3,
        Kind::Socket => 4,
        Kind::Opaque => 5,
    });
    e.word(entry.handle.rights() as u64);
    e.word(entry.handle.companions().len() as u64);
    for raw in entry.handle.companions() {
        e.word(*raw);
    }
    super::grant_codec::write(e, entry.handle.grant());
    e.word(entry.handle.allowed_methods().is_some() as u64);
    if let Some(methods) = entry.handle.allowed_methods() {
        e.word(methods.len() as u64);
        for n in methods {
            e.word(*n);
        }
    }
}

pub(super) fn read_entry(
    d: &mut Decoder<'_>,
    ownership: Arc<AtomicBool>,
    handles: &mut BTreeMap<u64, Arc<dyn Handle>>,
) -> Result<Entry, Error> {
    let name = d.text(4096)?.into();
    let raw = d.word()?;
    if raw == 0 {
        return Err(Error::InvalidData);
    }
    let kind = match d.word()? {
        1 => Kind::Channel,
        2 => Kind::Directory,
        3 => Kind::File,
        4 => Kind::Socket,
        5 => Kind::Opaque,
        _ => return Err(Error::InvalidData),
    };
    let rights = word32(d)?;
    if rights & !7 != 0 {
        return Err(Error::InvalidData);
    }
    let mut companions = Vec::new();
    for _ in 0..d.count(16)? {
        let h = d.word()?;
        if h == 0 {
            return Err(Error::InvalidData);
        }
        companions.push(h);
    }
    let grant = super::grant_codec::read(d)?;
    let allowed_methods = if d.flag()? {
        let mut methods = Vec::new();
        for _ in 0..d.count(256)? {
            methods.push(d.word()?);
        }
        Some(methods)
    } else {
        None
    };
    let handle = if let Some(h) = handles.get(&raw) {
        if h.kind() != kind
            || h.grant() != grant.as_ref()
            || h.rights() != rights
            || h.companions() != companions
            || h.allowed_methods() != allowed_methods.as_deref()
        {
            return Err(Error::InvalidData);
        }
        h.clone()
    } else {
        let h: Arc<dyn Handle> = Arc::new(NativeHandle {
            raw,
            kind,
            rights,
            companions,
            allowed_methods,
            grant,
            ownership: Some(ownership.clone()),
        });
        handles.insert(raw, h.clone());
        h
    };
    Ok(Entry { name, handle })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bexos_wasm_runtime::wasi::{
        checkpoint::Saved,
        state::{Descriptor, Input, Output, Pollable},
        wasi::filesystem::types::DescriptorFlags,
    };
    fn snapshot() -> Snapshot {
        let mut options = bexos_wasm_abi::WasmRunnerOptions::default();
        options.path = "/pkg/service.wasm".into();
        let handle: Arc<dyn Handle> = Arc::new(NativeHandle {
            raw: 91,
            kind: Kind::File,
            rights: 3,
            companions: Vec::new(),
            allowed_methods: None,
            grant: None,
            ownership: Some(Arc::new(AtomicBool::new(false))),
        });
        let entry = Entry {
            name: "open file".into(),
            handle,
        };
        Snapshot {
            insecure_seed: Some((123, 456)),
            options,
            origin: Origin::Signed,
            bytes: Arc::from(&b"\0asm\x01\0\0\0"[..]),
            checkpoint: vec![42],
            resources: vec![(7, entry.clone())],
            next_resource: 8,
            children: Vec::new(),
            next_child: 1,
            messages: Default::default(),
            remaining_fuel: 12345,
            paused: true,
            wasi: BTreeMap::from([
                (
                    0,
                    Saved::Descriptor(Descriptor {
                        entry: entry.clone(),
                        flags: DescriptorFlags::READ | DescriptorFlags::WRITE,
                    }),
                ),
                (4, Saved::Input(Input::File(entry.clone(), 123))),
                (9, Saved::Output(Output::file(entry, 456))),
                (15, Saved::Pollable(Pollable::Timer(789))),
            ]),
            component_dependencies: Vec::new(),
        }
    }
    #[test]
    fn preserves_wasi_offsets_ids_and_native_handle_aliases() {
        let encoded = encode(&snapshot()).unwrap();
        let restored = decode(&encoded, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(restored.checkpoint, [42]);
        assert_eq!(restored.remaining_fuel, 12345);
        assert_eq!(restored.insecure_seed, Some((123, 456)));
        assert!(restored.paused);
        let Saved::Descriptor(d) = restored.wasi.get(&0).unwrap() else {
            panic!("descriptor")
        };
        assert!(d.flags.contains(DescriptorFlags::WRITE));
        assert!(Arc::ptr_eq(
            &d.entry.handle,
            &restored.resources[0].1.handle
        ));
        assert!(matches!(
            restored.wasi.get(&4),
            Some(Saved::Input(Input::File(_, 123)))
        ));
        assert!(matches!(
            restored.wasi.get(&9),
            Some(Saved::Output(output)) if matches!(output.snapshot().target, bexos_wasm_runtime::wasi::output_state::Target::File(_, 456))
        ));
        assert!(matches!(
            restored.wasi.get(&15),
            Some(Saved::Pollable(Pollable::Timer(789)))
        ));
    }
    #[test]
    fn truncated_or_extra_checkpoint_bytes_are_rejected() {
        let encoded = encode(&snapshot()).unwrap();
        for length in 0..encoded.len() {
            assert!(decode(&encoded[..length], Arc::new(AtomicBool::new(false))).is_err());
        }
        let mut extra = encoded;
        extra.push(0);
        assert!(decode(&extra, Arc::new(AtomicBool::new(false))).is_err());
    }
}
