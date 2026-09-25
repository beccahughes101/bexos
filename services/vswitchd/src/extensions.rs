use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bexos_network_extension_abi::{
    ABI_VERSION, ActionCode, EXPORT_ABI_VERSION, EXPORT_BUFFER_CAPACITY, EXPORT_BUFFER_PTR,
    EXPORT_CHECKPOINT_LEN, EXPORT_CHECKPOINT_PTR, EXPORT_CONFIGURE, EXPORT_PROCESS, Hook,
    IMPORT_MODULE, IMPORT_MONOTONIC_NS, PacketAction, PacketDescriptor, VectorHeader, valid_action,
};
use wasmtime::{Engine, Linker, Memory, Module, Store, TypedFunc};

use crate::packet::{Packet, PacketDisposition};
use crate::switch::SwitchError;

const MAX_MODULE_BYTES: usize = 4 << 20;
const MAX_LINEAR_MEMORY: usize = 4 << 20;
const MAX_EXTENSIONS: usize = 8;
const MAX_TOTAL_STATE: usize = 16 << 20;
const FUEL_PER_VECTOR: u64 = 10_000_000;
const EPOCH_DEADLINE_TICKS: u64 = if cfg!(test) { 20 } else { 2 };

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailurePolicy {
    Closed,
    Open,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExtensionCounters {
    pub batches: u64,
    pub packets: u64,
    pub passed: u64,
    pub dropped: u64,
    pub rewritten: u64,
    pub redirected: u64,
    pub traps: u64,
}

struct Context;

pub struct Extension {
    pub name: String,
    pub hook: Hook,
    pub policy: FailurePolicy,
    pub digest: [u8; 32],
    pub module_bytes: Vec<u8>,
    pub config: Vec<u8>,
    pub generation: u64,
    pub counters: ExtensionCounters,
    pub faulted: bool,
    store: Store<Context>,
    memory: Memory,
    process: TypedFunc<i32, i32>,
    buffer_ptr: usize,
    buffer_capacity: usize,
    checkpoint_ptr: usize,
    checkpoint_len: usize,
    watchdog_stop: Arc<AtomicBool>,
}

impl Extension {
    pub fn compile(
        name: &str,
        hook: Hook,
        policy: FailurePolicy,
        digest: [u8; 32],
        bytes: &[u8],
        config: &[u8],
        generation: u64,
    ) -> Result<Self, SwitchError> {
        if name.is_empty()
            || name.len() > 64
            || bytes.len() < 8
            || bytes.len() > MAX_MODULE_BYTES
            || config.len() > bexos_network_extension_abi::MAX_CONFIG_BYTES
            || !bytes.starts_with(b"\0asm")
        {
            return Err(SwitchError::InvalidExtension);
        }
        let limits = bexos_wasm_abi::Limits {
            max_module_bytes: MAX_MODULE_BYTES as u64,
            max_memory_pages: (MAX_LINEAR_MEMORY / 65536) as u64,
            max_table_elements: 1024,
            max_stack_bytes: 128 << 10,
            max_handles: 1,
            max_children: 0,
            fuel: FUEL_PER_VECTOR,
            fuel_slice: 100_000,
        };
        let mut engine_config =
            bexos_wasm_engine::config(&limits).map_err(|_| SwitchError::InvalidExtension)?;
        engine_config.epoch_interruption(true);
        bexos_wasm_runtime::platform::configure(&mut engine_config, MAX_LINEAR_MEMORY);
        let engine = Engine::new(&engine_config).map_err(|_| SwitchError::InvalidExtension)?;
        let module = Module::new(&engine, bytes).map_err(|_| SwitchError::InvalidExtension)?;
        let mut linker = Linker::new(&engine);
        linker
            .func_wrap(IMPORT_MODULE, IMPORT_MONOTONIC_NS, || -> u64 {
                let ticks = bexos_userspace::syscall::ticks();
                let frequency = bexos_userspace::syscall::frequency().max(1);
                ((u128::from(ticks) * 1_000_000_000) / u128::from(frequency)) as u64
            })
            .map_err(|_| SwitchError::InvalidExtension)?;
        let mut store = Store::try_new_with_pulley_stack(&engine, Context, 128 << 10)
            .map_err(|_| SwitchError::InvalidExtension)?;
        store
            .set_fuel(FUEL_PER_VECTOR)
            .map_err(|_| SwitchError::InvalidExtension)?;
        // The epoch advances every 500 us. Production receives one complete
        // watchdog interval after the current partial tick. Host tests allow
        // scheduler jitter while retaining the same epoch source and fuel
        // enforcement.
        store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|_| SwitchError::InvalidExtension)?;
        let abi = instance
            .get_typed_func::<(), i32>(&mut store, EXPORT_ABI_VERSION)
            .and_then(|function| function.call(&mut store, ()))
            .map_err(|_| SwitchError::InvalidExtension)?;
        if abi != ABI_VERSION as i32 {
            return Err(SwitchError::InvalidExtension);
        }
        let buffer_ptr = instance
            .get_typed_func::<(), i32>(&mut store, EXPORT_BUFFER_PTR)
            .and_then(|function| function.call(&mut store, ()))
            .map_err(|_| SwitchError::InvalidExtension)? as usize;
        let buffer_capacity = instance
            .get_typed_func::<(), i32>(&mut store, EXPORT_BUFFER_CAPACITY)
            .and_then(|function| function.call(&mut store, ()))
            .map_err(|_| SwitchError::InvalidExtension)? as usize;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or(SwitchError::InvalidExtension)?;
        if buffer_capacity == 0
            || buffer_capacity > MAX_LINEAR_MEMORY
            || buffer_ptr.checked_add(buffer_capacity).is_none()
            || buffer_ptr + buffer_capacity > memory.data_size(&store)
        {
            return Err(SwitchError::InvalidExtension);
        }
        let configure = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, EXPORT_CONFIGURE)
            .map_err(|_| SwitchError::InvalidExtension)?;
        if config.len() > buffer_capacity {
            return Err(SwitchError::InvalidExtension);
        }
        memory
            .write(&mut store, buffer_ptr, config)
            .map_err(|_| SwitchError::InvalidExtension)?;
        if configure
            .call(&mut store, (buffer_ptr as i32, config.len() as i32))
            .map_err(|_| SwitchError::InvalidExtension)?
            != 0
        {
            return Err(SwitchError::InvalidExtension);
        }
        let process = instance
            .get_typed_func::<i32, i32>(&mut store, EXPORT_PROCESS)
            .map_err(|_| SwitchError::InvalidExtension)?;
        let checkpoint_ptr = instance
            .get_typed_func::<(), i32>(&mut store, EXPORT_CHECKPOINT_PTR)
            .and_then(|function| function.call(&mut store, ()))
            .map_err(|_| SwitchError::InvalidExtension)? as usize;
        let checkpoint_len = instance
            .get_typed_func::<(), i32>(&mut store, EXPORT_CHECKPOINT_LEN)
            .and_then(|function| function.call(&mut store, ()))
            .map_err(|_| SwitchError::InvalidExtension)? as usize;
        if checkpoint_len > MAX_LINEAR_MEMORY
            || checkpoint_ptr.checked_add(checkpoint_len).is_none()
            || checkpoint_ptr + checkpoint_len > memory.data_size(&store)
        {
            return Err(SwitchError::InvalidExtension);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_engine = engine.clone();
        std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                std::thread::sleep(core::time::Duration::from_micros(500));
                worker_engine.increment_epoch();
            }
        });
        Ok(Self {
            name: name.to_string(),
            hook,
            policy,
            digest,
            module_bytes: bytes.to_vec(),
            config: config.to_vec(),
            generation,
            counters: ExtensionCounters::default(),
            faulted: false,
            store,
            memory,
            process,
            buffer_ptr,
            buffer_capacity,
            checkpoint_ptr,
            checkpoint_len,
            watchdog_stop: stop,
        })
    }

    pub fn process(&mut self, hook: Hook, packets: &mut [Packet]) {
        if self.hook != hook || packets.is_empty() {
            return;
        }
        if self.faulted {
            apply_failure(self.policy, packets, &mut self.counters);
            return;
        }
        self.counters.batches = self.counters.batches.saturating_add(1);
        self.counters.packets = self.counters.packets.saturating_add(packets.len() as u64);
        if self.process_inner(hook, packets).is_err() {
            self.faulted = true;
            self.counters.traps = self.counters.traps.saturating_add(1);
            apply_failure(self.policy, packets, &mut self.counters);
        }
    }

    fn process_inner(&mut self, hook: Hook, packets: &mut [Packet]) -> Result<(), SwitchError> {
        let packet_bytes = packets.iter().try_fold(0usize, |total, packet| {
            total
                .checked_add(packet.bytes.len())
                .ok_or(SwitchError::InvalidExtension)
        })?;
        let mut header =
            VectorHeader::checked_layout(packets.len(), packet_bytes, self.buffer_capacity)
                .ok_or(SwitchError::InvalidExtension)?;
        header.hook = hook as u8;
        let mut image = vec![
            0u8;
            self.buffer_capacity
                .min(header.packets_offset as usize + packet_bytes,)
        ];
        encode_header(&header, &mut image[..core::mem::size_of::<VectorHeader>()]);
        let mut packet_offset = 0usize;
        for (index, packet) in packets.iter().enumerate() {
            let mut descriptor = packet.descriptor;
            descriptor.packet_offset = packet_offset as u32;
            descriptor.length = packet.bytes.len() as u32;
            let start = header.descriptors_offset as usize
                + index * core::mem::size_of::<PacketDescriptor>();
            encode_descriptor(&descriptor, &mut image[start..start + 48]);
            let action =
                header.actions_offset as usize + index * core::mem::size_of::<PacketAction>();
            image[action..action + 2].copy_from_slice(&(ActionCode::Pass as u16).to_le_bytes());
            let data = header.packets_offset as usize + packet_offset;
            image[data..data + packet.bytes.len()].copy_from_slice(&packet.bytes);
            packet_offset += packet.bytes.len();
        }
        self.memory
            .write(&mut self.store, self.buffer_ptr, &image)
            .map_err(|_| SwitchError::InvalidExtension)?;
        self.store
            .set_fuel(FUEL_PER_VECTOR)
            .map_err(|_| SwitchError::InvalidExtension)?;
        self.store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
        if self
            .process
            .call(&mut self.store, self.buffer_ptr as i32)
            .map_err(|_| SwitchError::InvalidExtension)?
            != 0
        {
            return Err(SwitchError::InvalidExtension);
        }
        self.memory
            .read(&self.store, self.buffer_ptr, &mut image)
            .map_err(|_| SwitchError::InvalidExtension)?;
        packet_offset = 0;
        for (index, packet) in packets.iter_mut().enumerate() {
            let action_offset =
                header.actions_offset as usize + index * core::mem::size_of::<PacketAction>();
            let code =
                u16::from_le_bytes(image[action_offset..action_offset + 2].try_into().unwrap());
            if !valid_action(code) {
                return Err(SwitchError::InvalidExtension);
            }
            match code {
                value if value == ActionCode::Drop as u16 => {
                    packet.disposition = PacketDisposition::Drop;
                    self.counters.dropped = self.counters.dropped.saturating_add(1);
                }
                value if value == ActionCode::Pass as u16 => {
                    self.counters.passed = self.counters.passed.saturating_add(1);
                }
                value if value == ActionCode::Rewrite as u16 => {
                    let data = header.packets_offset as usize + packet_offset;
                    let length = packet.bytes.len();
                    packet.bytes.copy_from_slice(&image[data..data + length]);
                    packet.rewritten = true;
                    self.counters.rewritten = self.counters.rewritten.saturating_add(1);
                }
                _ => {
                    let redirect = u64::from_le_bytes(
                        image[action_offset + 8..action_offset + 16]
                            .try_into()
                            .unwrap(),
                    );
                    if redirect == 0 {
                        return Err(SwitchError::InvalidExtension);
                    }
                    // REDIRECT names a routed interface. The graph validates
                    // its VRF before the output node turns it into a physical
                    // or virtual delivery disposition.
                    packet.descriptor.egress_interface = redirect;
                    self.counters.redirected = self.counters.redirected.saturating_add(1);
                }
            }
            packet_offset += packet.bytes.len();
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.memory.data(&self.store)
            [self.checkpoint_ptr..self.checkpoint_ptr + self.checkpoint_len]
            .to_vec()
    }

    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), SwitchError> {
        if bytes.len() != self.checkpoint_len {
            return Err(SwitchError::InvalidExtension);
        }
        self.memory
            .write(&mut self.store, self.checkpoint_ptr, bytes)
            .map_err(|_| SwitchError::InvalidExtension)
    }
}

impl Drop for Extension {
    fn drop(&mut self) {
        self.watchdog_stop.store(true, Ordering::Release);
    }
}

#[derive(Default)]
pub struct ExtensionSet {
    extensions: Vec<Extension>,
}

impl ExtensionSet {
    pub fn install(&mut self, extension: Extension) -> Result<(), SwitchError> {
        if !self.extensions.iter().any(|old| old.name == extension.name)
            && self.extensions.len() >= MAX_EXTENSIONS
        {
            return Err(SwitchError::QueueFull);
        }
        let old_state = self
            .extensions
            .iter()
            .map(|entry| entry.memory.data_size(&entry.store))
            .sum::<usize>();
        let replaced = self
            .extensions
            .iter()
            .find(|old| old.name == extension.name)
            .map_or(0, |old| old.memory.data_size(&old.store));
        if old_state
            .saturating_sub(replaced)
            .saturating_add(extension.memory.data_size(&extension.store))
            > MAX_TOTAL_STATE
        {
            return Err(SwitchError::QueueFull);
        }
        if let Some(index) = self
            .extensions
            .iter()
            .position(|old| old.name == extension.name)
        {
            self.extensions[index] = extension;
        } else {
            self.extensions.push(extension);
        }
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<Extension, SwitchError> {
        let index = self
            .extensions
            .iter()
            .position(|entry| entry.name == name)
            .ok_or(SwitchError::NotFound)?;
        Ok(self.extensions.remove(index))
    }

    /// Removes the active pre- and post-routing NAT views as one graph
    /// transaction. The requested name must identify one of the active views.
    pub fn remove_nat_pair(&mut self, name: &str) -> Result<Vec<Extension>, SwitchError> {
        if !self.extensions.iter().any(|entry| {
            entry.name == name && matches!(entry.hook, Hook::PreRouting | Hook::PostRouting)
        }) {
            return Err(SwitchError::NotFound);
        }
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.extensions.len() {
            if matches!(
                self.extensions[index].hook,
                Hook::PreRouting | Hook::PostRouting
            ) {
                removed.push(self.extensions.remove(index));
            } else {
                index += 1;
            }
        }
        Ok(removed)
    }

    pub fn replace_hook(&mut self, extension: Extension) -> Result<Vec<Extension>, SwitchError> {
        let additional = extension.memory.data_size(&extension.store);
        let retained = self
            .extensions
            .iter()
            .filter(|old| old.hook != extension.hook)
            .map(|old| old.memory.data_size(&old.store))
            .sum::<usize>();
        if retained.saturating_add(additional) > MAX_TOTAL_STATE {
            return Err(SwitchError::QueueFull);
        }
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.extensions.len() {
            if self.extensions[index].hook == extension.hook {
                removed.push(self.extensions.remove(index));
            } else {
                index += 1;
            }
        }
        self.extensions.push(extension);
        Ok(removed)
    }

    /// Installs the two views of one stateful NAT module as a single graph
    /// transaction. Both modules are compiled and configured before this is
    /// called, so a failure can never leave only one side of translation
    /// active.
    pub fn replace_nat_pair(
        &mut self,
        extensions: [Extension; 2],
    ) -> Result<Vec<Extension>, SwitchError> {
        if extensions[0].hook != Hook::PreRouting || extensions[1].hook != Hook::PostRouting {
            return Err(SwitchError::InvalidExtension);
        }
        let retained = self
            .extensions
            .iter()
            .filter(|old| !matches!(old.hook, Hook::PreRouting | Hook::PostRouting))
            .map(|old| old.memory.data_size(&old.store))
            .sum::<usize>();
        let additional = extensions
            .iter()
            .map(|extension| extension.memory.data_size(&extension.store))
            .sum::<usize>();
        if retained.saturating_add(additional) > MAX_TOTAL_STATE {
            return Err(SwitchError::QueueFull);
        }
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.extensions.len() {
            if matches!(
                self.extensions[index].hook,
                Hook::PreRouting | Hook::PostRouting
            ) {
                removed.push(self.extensions.remove(index));
            } else {
                index += 1;
            }
        }
        self.extensions.extend(extensions);
        Ok(removed)
    }

    pub fn process(&mut self, hook: Hook, packets: &mut [Packet]) {
        let matching = self.extensions.iter().position(|entry| entry.hook == hook);
        let Some(index) = matching else { return };
        self.extensions[index].process(hook, packets);

        // NAT has one logical state table even though the graph invokes it at
        // two different hooks. Keep the exported durable checkpoint identical
        // after each vector; this also makes either side sufficient for heart
        // transplant recovery.
        if matches!(hook, Hook::PreRouting | Hook::PostRouting) {
            let state = self.extensions[index].snapshot();
            let peer_hook = if hook == Hook::PreRouting {
                Hook::PostRouting
            } else {
                Hook::PreRouting
            };
            if let Some(peer) = self
                .extensions
                .iter_mut()
                .find(|entry| entry.hook == peer_hook)
            {
                if peer.restore(&state).is_err() {
                    peer.faulted = true;
                    peer.counters.traps = peer.counters.traps.saturating_add(1);
                }
            }
        }
    }

    pub fn process_indices(&mut self, hook: Hook, packets: &mut [Packet], indices: &[usize]) {
        if indices.is_empty() {
            return;
        }
        let mut vector = indices
            .iter()
            .map(|index| packets[*index].clone())
            .collect::<Vec<_>>();
        self.process(hook, &mut vector);
        for (index, packet) in indices.iter().zip(vector) {
            packets[*index] = packet;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Extension> {
        self.extensions.iter()
    }

    pub fn clear(&mut self) {
        self.extensions.clear();
    }
}

fn apply_failure(policy: FailurePolicy, packets: &mut [Packet], counters: &mut ExtensionCounters) {
    if policy == FailurePolicy::Closed {
        for packet in packets {
            packet.disposition = PacketDisposition::Drop;
            counters.dropped = counters.dropped.saturating_add(1);
        }
    } else {
        counters.passed = counters.passed.saturating_add(packets.len() as u64);
    }
}

fn encode_header(header: &VectorHeader, out: &mut [u8]) {
    out.fill(0);
    out[0..4].copy_from_slice(&header.abi_version.to_le_bytes());
    out[4] = header.hook;
    out[8..12].copy_from_slice(&header.count.to_le_bytes());
    out[12..16].copy_from_slice(&header.descriptors_offset.to_le_bytes());
    out[16..20].copy_from_slice(&header.actions_offset.to_le_bytes());
    out[20..24].copy_from_slice(&header.packets_offset.to_le_bytes());
    out[24..28].copy_from_slice(&header.packets_length.to_le_bytes());
}

fn encode_descriptor(descriptor: &PacketDescriptor, out: &mut [u8]) {
    out.fill(0);
    out[0..4].copy_from_slice(&descriptor.packet_offset.to_le_bytes());
    out[4..8].copy_from_slice(&descriptor.length.to_le_bytes());
    out[8..16].copy_from_slice(&descriptor.ingress_interface.to_le_bytes());
    out[16..24].copy_from_slice(&descriptor.egress_interface.to_le_bytes());
    out[24..28].copy_from_slice(&descriptor.table_id.to_le_bytes());
    out[28..32].copy_from_slice(&descriptor.flow_hash.to_le_bytes());
    for (index, value) in [
        descriptor.l2_offset,
        descriptor.l3_offset,
        descriptor.l4_offset,
        descriptor.vlan_id,
        descriptor.source_zone,
        descriptor.destination_zone,
    ]
    .iter()
    .enumerate()
    {
        let offset = 32 + index * 2;
        out[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    out[44] = descriptor.direction;
    out[45] = descriptor.ip_version;
    out[46] = descriptor.protocol;
    out[47] = descriptor.fragment_flags;
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use bexos_network_extension_abi::Direction;

    use super::*;
    use crate::packet::Packet;

    #[test]
    fn traps_and_invalid_actions_quarantine_with_declared_policy() {
        for (body, policy, expected) in [
            (
                "unreachable",
                FailurePolicy::Closed,
                PacketDisposition::Drop,
            ),
            (
                "unreachable",
                FailurePolicy::Open,
                PacketDisposition::Continue,
            ),
            (
                "local.get 0 i32.const 16 i32.add i32.load i32.const 99 i32.store16 i32.const 0",
                FailurePolicy::Closed,
                PacketDisposition::Drop,
            ),
        ] {
            let bytes = module(body, "");
            let mut extension =
                Extension::compile("test", Hook::Firewall, policy, [1; 32], &bytes, &[], 1)
                    .unwrap();
            let mut packets = vec![packet()];
            extension.process(Hook::Firewall, &mut packets);
            assert!(extension.faulted);
            assert_eq!(extension.counters.traps, 1);
            assert_eq!(packets[0].disposition, expected);
        }
    }

    #[test]
    fn fuel_interrupts_a_non_terminating_module() {
        let bytes = module("(loop br 0) i32.const 0", "");
        let mut extension = Extension::compile(
            "loop",
            Hook::Firewall,
            FailurePolicy::Closed,
            [2; 32],
            &bytes,
            &[],
            1,
        )
        .unwrap();
        let mut packets = vec![packet()];
        extension.process(Hook::Firewall, &mut packets);
        assert!(extension.faulted);
        assert_eq!(packets[0].disposition, PacketDisposition::Drop);
    }

    #[test]
    fn undeclared_import_and_failed_replacement_leave_old_instance_active() {
        let invalid = module("i32.const 0", "(import \"env\" \"ambient\" (func))");
        assert!(
            Extension::compile(
                "bad",
                Hook::Firewall,
                FailurePolicy::Closed,
                [3; 32],
                &invalid,
                &[],
                2,
            )
            .is_err()
        );

        let mut set = ExtensionSet::default();
        set.install(
            Extension::compile(
                "old",
                Hook::Firewall,
                FailurePolicy::Closed,
                [4; 32],
                &module("i32.const 0", ""),
                &[],
                1,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(set.iter().next().unwrap().generation, 1);
        assert_eq!(set.iter().count(), 1);
    }

    #[test]
    fn nat_pair_removal_is_atomic() {
        let mut set = ExtensionSet::default();
        let pre = Extension::compile(
            "nat.pre-routing",
            Hook::PreRouting,
            FailurePolicy::Closed,
            [5; 32],
            &module("i32.const 0", ""),
            &[],
            7,
        )
        .unwrap();
        let post = Extension::compile(
            "nat.post-routing",
            Hook::PostRouting,
            FailurePolicy::Closed,
            [5; 32],
            &module("i32.const 0", ""),
            &[],
            7,
        )
        .unwrap();
        set.replace_nat_pair([pre, post]).unwrap();

        let removed = set.remove_nat_pair("nat.pre-routing").unwrap();
        assert_eq!(removed.len(), 2);
        assert!(
            set.iter()
                .all(|extension| !matches!(extension.hook, Hook::PreRouting | Hook::PostRouting))
        );
        assert!(matches!(
            set.remove_nat_pair("nat.post-routing"),
            Err(SwitchError::NotFound)
        ));
    }

    fn module(process_body: &str, imports: &str) -> Vec<u8> {
        wat::parse_str(alloc::format!(
            r#"(module
            {imports}
            (memory (export "memory") 1)
            (func (export "bexos_extension_abi_version") (result i32) i32.const 1)
            (func (export "bexos_extension_buffer_ptr") (result i32) i32.const 0)
            (func (export "bexos_extension_buffer_capacity") (result i32) i32.const 65536)
            (func (export "bexos_extension_configure") (param i32 i32) (result i32) i32.const 0)
            (func (export "bexos_extension_process_vector") (param i32) (result i32) {process_body})
            (func (export "bexos_extension_checkpoint_ptr") (result i32) i32.const 65536)
            (func (export "bexos_extension_checkpoint_len") (result i32) i32.const 0)
        )"#
        ))
        .unwrap()
    }

    fn packet() -> Packet {
        let mut bytes = vec![0u8; 34];
        bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        bytes[14] = 0x45;
        bytes[16..18].copy_from_slice(&20u16.to_be_bytes());
        bytes[22] = 64;
        bytes[23] = 17;
        bytes[26..30].copy_from_slice(&[10, 0, 0, 1]);
        bytes[30..34].copy_from_slice(&[10, 0, 0, 2]);
        let mut sum = bytes[14..34]
            .chunks(2)
            .map(|word| u32::from(u16::from_be_bytes([word[0], word[1]])))
            .sum::<u32>();
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        bytes[24..26].copy_from_slice(&(!(sum as u16)).to_be_bytes());
        Packet::parse(&bytes, 1, Direction::VirtualIngress, 1, 1).unwrap()
    }
}
