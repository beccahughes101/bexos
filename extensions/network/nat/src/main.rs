#![no_std]
#![no_main]

use bexos_network_extension_abi::{
    ABI_VERSION, ActionCode, Direction, Hook, MAX_NAT_MAPPINGS, PacketAction, PacketDescriptor,
};
use bexos_network_extension_guest::{
    Tuple, VECTOR_BUFFER_CAPACITY, VectorView, fragment, replace_address, replace_checksum_word,
    tuple,
};
use core::panic::PanicInfo;

const MAPPING_TIMEOUT_NS: u64 = 120_000_000_000;
const CONFIG_HEADER_BYTES: usize = 48;
const FORWARD_BYTES: usize = 16;
const MAX_FORWARDS: usize = 128;
const MAX_FRAGMENT_ASSOCIATIONS: usize = 256;

#[link(wasm_import_module = "bexos:net/extension@1.0.0")]
unsafe extern "C" {
    #[link_name = "monotonic-ns"]
    fn monotonic_ns() -> u64;
}

#[derive(Clone, Copy)]
struct Config {
    external_v4: [u8; 4],
    port_start: u16,
    port_end: u16,
    internal_v6: [u8; 16],
    external_v6: [u8; 16],
    prefix_len: u8,
}

const EMPTY_CONFIG: Config = Config {
    external_v4: [0; 4],
    port_start: 49152,
    port_end: 65535,
    internal_v6: [0; 16],
    external_v6: [0; 16],
    prefix_len: 0,
};

#[derive(Clone, Copy)]
struct Forward {
    external_address: [u8; 4],
    external_port: u16,
    internal_address: [u8; 4],
    internal_port: u16,
    protocol: u8,
}
const EMPTY_FORWARD: Forward = Forward {
    external_address: [0; 4],
    external_port: 0,
    internal_address: [0; 4],
    internal_port: 0,
    protocol: 0,
};

#[derive(Clone, Copy)]
struct Mapping {
    used: bool,
    protocol: u8,
    internal_address: [u8; 4],
    remote_address: [u8; 4],
    internal_port: u16,
    external_port: u16,
    remote_port: u16,
    expires_ns: u64,
}

const EMPTY_MAPPING: Mapping = Mapping {
    used: false,
    protocol: 0,
    internal_address: [0; 4],
    remote_address: [0; 4],
    internal_port: 0,
    external_port: 0,
    remote_port: 0,
    expires_ns: 0,
};

#[derive(Clone, Copy)]
struct FragmentAssociation {
    used: bool,
    protocol: u8,
    hook: u8,
    source_rewrite: bool,
    source: [u8; 4],
    destination: [u8; 4],
    translated: [u8; 4],
    id: u32,
    expires_ns: u64,
}

const EMPTY_FRAGMENT: FragmentAssociation = FragmentAssociation {
    used: false,
    protocol: 0,
    hook: 0,
    source_rewrite: false,
    source: [0; 4],
    destination: [0; 4],
    translated: [0; 4],
    id: 0,
    expires_ns: 0,
};

#[repr(C)]
struct Checkpoint {
    next_port: u16,
    reserved: u16,
    high_water: u32,
    fragment_high_water: u32,
    padding: u32,
    mappings: [Mapping; MAX_NAT_MAPPINGS],
    fragments: [FragmentAssociation; MAX_FRAGMENT_ASSOCIATIONS],
}

static mut BUFFER: core::mem::MaybeUninit<[u8; VECTOR_BUFFER_CAPACITY]> =
    core::mem::MaybeUninit::uninit();
static mut CONFIG: Config = EMPTY_CONFIG;
static mut FORWARDS: [Forward; MAX_FORWARDS] = [EMPTY_FORWARD; MAX_FORWARDS];
static mut FORWARD_COUNT: usize = 0;
static mut CHECKPOINT: Checkpoint = Checkpoint {
    next_port: 49152,
    reserved: 0,
    high_water: 0,
    fragment_high_water: 0,
    padding: 0,
    mappings: [EMPTY_MAPPING; MAX_NAT_MAPPINGS],
    fragments: [EMPTY_FRAGMENT; MAX_FRAGMENT_ASSOCIATIONS],
};

#[unsafe(no_mangle)]
pub extern "C" fn bexos_extension_abi_version() -> i32 {
    ABI_VERSION as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn bexos_extension_buffer_ptr() -> i32 {
    core::ptr::addr_of_mut!(BUFFER) as *mut u8 as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn bexos_extension_buffer_capacity() -> i32 {
    VECTOR_BUFFER_CAPACITY as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn bexos_extension_checkpoint_ptr() -> i32 {
    core::ptr::addr_of_mut!(CHECKPOINT) as *mut Checkpoint as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn bexos_extension_checkpoint_len() -> i32 {
    core::mem::size_of::<Checkpoint>() as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_extension_configure(pointer: i32, length: i32) -> i32 {
    if pointer < 0
        || length < CONFIG_HEADER_BYTES as i32
        || length as usize > CONFIG_HEADER_BYTES + MAX_FORWARDS * FORWARD_BYTES
    {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length as usize) };
    if &bytes[..4] != b"NAT1" {
        return -1;
    }
    let config = Config {
        external_v4: bytes[4..8].try_into().unwrap(),
        port_start: u16::from_le_bytes([bytes[8], bytes[9]]),
        port_end: u16::from_le_bytes([bytes[10], bytes[11]]),
        prefix_len: bytes[12],
        internal_v6: bytes[16..32].try_into().unwrap(),
        external_v6: bytes[32..48].try_into().unwrap(),
    };
    let count = u16::from_le_bytes([bytes[14], bytes[15]]) as usize;
    if bytes.len() != CONFIG_HEADER_BYTES + count * FORWARD_BYTES {
        return -1;
    }
    if config.external_v4 == [0; 4]
        || config.port_start == 0
        || config.port_start > config.port_end
        || config.prefix_len > 112
    {
        return -1;
    }
    unsafe {
        CONFIG = config;
        CHECKPOINT.next_port = config.port_start;
        CHECKPOINT.high_water = 0;
        CHECKPOINT.fragment_high_water = 0;
        FORWARD_COUNT = count;
        for index in 0..count {
            let start = CONFIG_HEADER_BYTES + index * FORWARD_BYTES;
            let forward = Forward {
                external_address: bytes[start..start + 4].try_into().unwrap(),
                external_port: u16::from_le_bytes([bytes[start + 4], bytes[start + 5]]),
                internal_address: bytes[start + 6..start + 10].try_into().unwrap(),
                internal_port: u16::from_le_bytes([bytes[start + 10], bytes[start + 11]]),
                protocol: bytes[start + 12],
            };
            if forward.external_address == [0; 4]
                || forward.internal_address == [0; 4]
                || forward.external_port == 0
                || forward.internal_port == 0
                || !matches!(forward.protocol, 6 | 17)
            {
                return -1;
            }
            core::ptr::addr_of_mut!(FORWARDS[index]).write(forward);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_extension_process_vector(pointer: i32) -> i32 {
    if pointer != bexos_extension_buffer_ptr() {
        return -1;
    }
    let Some(mut vector) =
        (unsafe { VectorView::parse(pointer as *mut u8, VECTOR_BUFFER_CAPACITY) })
    else {
        return -1;
    };
    let hook = vector.hook();
    let now = unsafe { monotonic_ns() };
    for index in 0..vector.count() {
        let Some(descriptor) = (unsafe { vector.descriptor(index) }) else {
            return -1;
        };
        let action = if let Some(packet) = unsafe { vector.packet_mut(descriptor) } {
            translate(packet, descriptor, hook, now)
        } else {
            ActionCode::Drop
        };
        unsafe {
            vector.set_action(
                index,
                PacketAction {
                    code: action as u16,
                    flags: 0,
                    redirect_interface: 0,
                },
            );
        }
    }
    0
}

fn translate(packet: &mut [u8], descriptor: PacketDescriptor, hook: u8, now: u64) -> ActionCode {
    let Some(tuple) = tuple(packet, descriptor) else {
        return ActionCode::Drop;
    };
    let fragment = fragment(packet, descriptor);
    if tuple.version == 6 {
        return translate_nptv6(packet, descriptor, tuple, hook);
    }
    if tuple.version != 4 {
        return ActionCode::Drop;
    }
    if let Some(fragment) = fragment.filter(|fragment| fragment.offset != 0) {
        return translate_fragment(packet, descriptor, tuple, hook, fragment.id, now);
    }
    let action = if hook == Hook::PostRouting as u8
        && descriptor.direction == Direction::VirtualIngress as u8
    {
        translate_outbound(packet, descriptor, tuple, now)
    } else if hook == Hook::PreRouting as u8 {
        translate_inbound(packet, descriptor, tuple, now)
    } else {
        ActionCode::Pass
    };
    if action == ActionCode::Rewrite {
        if let Some(fragment) = fragment.filter(|fragment| fragment.more) {
            let l3 = descriptor.l3_offset as usize;
            let source_rewrite = hook == Hook::PostRouting as u8;
            let translated = if source_rewrite {
                packet[l3 + 12..l3 + 16].try_into().unwrap()
            } else {
                packet[l3 + 16..l3 + 20].try_into().unwrap()
            };
            if !record_fragment(tuple, hook, source_rewrite, translated, fragment.id, now) {
                return ActionCode::Drop;
            }
        }
    }
    action
}

fn translate_fragment(
    packet: &mut [u8],
    descriptor: PacketDescriptor,
    tuple: Tuple,
    hook: u8,
    id: u32,
    now: u64,
) -> ActionCode {
    let source: [u8; 4] = tuple.source[..4].try_into().unwrap();
    let destination: [u8; 4] = tuple.destination[..4].try_into().unwrap();
    let high = unsafe { CHECKPOINT.fragment_high_water as usize }.min(MAX_FRAGMENT_ASSOCIATIONS);
    let association = (0..high).find_map(|index| {
        let entry = unsafe { core::ptr::addr_of!(CHECKPOINT.fragments[index]).read() };
        (entry.used
            && entry.expires_ns >= now
            && entry.protocol == tuple.protocol
            && entry.hook == hook
            && entry.source == source
            && entry.destination == destination
            && entry.id == id)
            .then_some(entry)
    });
    let Some(association) = association else {
        return ActionCode::Drop;
    };
    let old = if association.source_rewrite {
        source
    } else {
        destination
    };
    rewrite_v4_header_address(
        packet,
        descriptor,
        association.source_rewrite,
        old,
        association.translated,
    );
    ActionCode::Rewrite
}

fn record_fragment(
    tuple: Tuple,
    hook: u8,
    source_rewrite: bool,
    translated: [u8; 4],
    id: u32,
    now: u64,
) -> bool {
    let high = unsafe { CHECKPOINT.fragment_high_water as usize }.min(MAX_FRAGMENT_ASSOCIATIONS);
    let index = (0..high)
        .find(|index| {
            let entry = unsafe { core::ptr::addr_of!(CHECKPOINT.fragments[*index]).read() };
            !entry.used || entry.expires_ns < now
        })
        .or_else(|| (high < MAX_FRAGMENT_ASSOCIATIONS).then_some(high));
    let Some(index) = index else { return false };
    unsafe {
        core::ptr::addr_of_mut!(CHECKPOINT.fragments[index]).write(FragmentAssociation {
            used: true,
            protocol: tuple.protocol,
            hook,
            source_rewrite,
            source: tuple.source[..4].try_into().unwrap(),
            destination: tuple.destination[..4].try_into().unwrap(),
            translated,
            id,
            expires_ns: now.saturating_add(MAPPING_TIMEOUT_NS),
        });
        if index == high {
            CHECKPOINT.fragment_high_water = (high + 1) as u32;
        }
    }
    true
}

fn translate_outbound(
    packet: &mut [u8],
    descriptor: PacketDescriptor,
    tuple: Tuple,
    now: u64,
) -> ActionCode {
    let internal: [u8; 4] = tuple.source[..4].try_into().unwrap();
    let remote: [u8; 4] = tuple.destination[..4].try_into().unwrap();
    let mapping = find_outbound(tuple, now).or_else(|| allocate(tuple, internal, remote, now));
    let Some(mapping) = mapping else {
        return ActionCode::Drop;
    };
    let config = unsafe { CONFIG };
    rewrite_v4_address(packet, descriptor, true, internal, config.external_v4);
    rewrite_port(
        packet,
        descriptor,
        tuple.protocol,
        true,
        tuple.source_port,
        mapping.external_port,
    );
    ActionCode::Rewrite
}

fn translate_inbound(
    packet: &mut [u8],
    descriptor: PacketDescriptor,
    tuple: Tuple,
    now: u64,
) -> ActionCode {
    let config = unsafe { CONFIG };
    if tuple.destination[..4] != config.external_v4 && find_forward(tuple).is_none() {
        return ActionCode::Pass;
    }
    let high_water = unsafe { CHECKPOINT.high_water as usize }.min(MAX_NAT_MAPPINGS);
    let mapping = (0..high_water).find_map(|index| {
        let mapping = unsafe { core::ptr::addr_of!(CHECKPOINT.mappings[index]).read() };
        let external_port = if tuple.protocol == 1 {
            tuple.source_port
        } else {
            tuple.destination_port
        };
        (mapping.used
            && mapping.expires_ns >= now
            && mapping.protocol == tuple.protocol
            && mapping.external_port == external_port
            && mapping.remote_address == tuple.source[..4]
            && (tuple.protocol == 1 || mapping.remote_port == tuple.source_port))
            .then_some(mapping)
    });
    let mapping = mapping.or_else(|| {
        let forward = find_forward(tuple)?;
        record_forward(tuple, forward, now)
    });
    let Some(mapping) = mapping else {
        return ActionCode::Drop;
    };
    rewrite_v4_address(
        packet,
        descriptor,
        false,
        tuple.destination[..4].try_into().unwrap(),
        mapping.internal_address,
    );
    rewrite_port(
        packet,
        descriptor,
        tuple.protocol,
        false,
        mapping.external_port,
        mapping.internal_port,
    );
    ActionCode::Rewrite
}

fn find_forward(tuple: Tuple) -> Option<Forward> {
    (0..unsafe { FORWARD_COUNT }).find_map(|index| {
        let forward = unsafe { core::ptr::addr_of!(FORWARDS[index]).read() };
        (forward.external_address == tuple.destination[..4]
            && forward.external_port == tuple.destination_port
            && forward.protocol == tuple.protocol)
            .then_some(forward)
    })
}

fn record_forward(tuple: Tuple, forward: Forward, now: u64) -> Option<Mapping> {
    let index = mapping_slot(now)?;
    let mapping = Mapping {
        used: true,
        protocol: tuple.protocol,
        internal_address: forward.internal_address,
        remote_address: tuple.source[..4].try_into().unwrap(),
        internal_port: forward.internal_port,
        external_port: forward.external_port,
        remote_port: tuple.source_port,
        expires_ns: now.saturating_add(MAPPING_TIMEOUT_NS),
    };
    unsafe { core::ptr::addr_of_mut!(CHECKPOINT.mappings[index]).write(mapping) };
    Some(mapping)
}

fn find_outbound(tuple: Tuple, now: u64) -> Option<Mapping> {
    let high_water = unsafe { CHECKPOINT.high_water as usize }.min(MAX_NAT_MAPPINGS);
    (0..high_water).find_map(|index| {
        let mut mapping = unsafe { core::ptr::addr_of!(CHECKPOINT.mappings[index]).read() };
        if mapping.used
            && mapping.expires_ns >= now
            && mapping.protocol == tuple.protocol
            && mapping.internal_address == tuple.source[..4]
            && mapping.remote_address == tuple.destination[..4]
            && mapping.internal_port == tuple.source_port
            && mapping.remote_port == tuple.destination_port
        {
            mapping.expires_ns = now.saturating_add(MAPPING_TIMEOUT_NS);
            unsafe { core::ptr::addr_of_mut!(CHECKPOINT.mappings[index]).write(mapping) };
            Some(mapping)
        } else {
            None
        }
    })
}

fn allocate(tuple: Tuple, internal: [u8; 4], remote: [u8; 4], now: u64) -> Option<Mapping> {
    let config = unsafe { CONFIG };
    let slots = u32::from(config.port_end) - u32::from(config.port_start) + 1;
    for _ in 0..slots {
        let port = unsafe { CHECKPOINT.next_port };
        unsafe {
            CHECKPOINT.next_port = if port == config.port_end {
                config.port_start
            } else {
                port + 1
            };
        }
        let high_water = unsafe { CHECKPOINT.high_water as usize }.min(MAX_NAT_MAPPINGS);
        if (0..high_water).any(|index| {
            let mapping = unsafe { core::ptr::addr_of!(CHECKPOINT.mappings[index]).read() };
            mapping.used
                && mapping.expires_ns >= now
                && mapping.protocol == tuple.protocol
                && mapping.external_port == port
        }) {
            continue;
        }
        if let Some(index) = mapping_slot(now) {
            let mapping = Mapping {
                used: true,
                protocol: tuple.protocol,
                internal_address: internal,
                remote_address: remote,
                internal_port: tuple.source_port,
                external_port: port,
                remote_port: tuple.destination_port,
                expires_ns: now.saturating_add(MAPPING_TIMEOUT_NS),
            };
            unsafe { core::ptr::addr_of_mut!(CHECKPOINT.mappings[index]).write(mapping) };
            return Some(mapping);
        }
        return None;
    }
    None
}

fn mapping_slot(now: u64) -> Option<usize> {
    let high_water = unsafe { CHECKPOINT.high_water as usize }.min(MAX_NAT_MAPPINGS);
    if let Some(index) = (0..high_water).find(|index| {
        let mapping = unsafe { core::ptr::addr_of!(CHECKPOINT.mappings[*index]).read() };
        !mapping.used || mapping.expires_ns < now
    }) {
        return Some(index);
    }
    if high_water == MAX_NAT_MAPPINGS {
        return None;
    }
    unsafe { CHECKPOINT.high_water = (high_water + 1) as u32 };
    Some(high_water)
}

fn translate_nptv6(
    packet: &mut [u8],
    descriptor: PacketDescriptor,
    tuple: Tuple,
    hook: u8,
) -> ActionCode {
    let config = unsafe { CONFIG };
    if config.prefix_len == 0 {
        return ActionCode::Pass;
    }
    let outbound =
        hook == Hook::PostRouting as u8 && descriptor.direction == Direction::VirtualIngress as u8;
    let inbound =
        hook == Hook::PreRouting as u8 && descriptor.direction == Direction::PhysicalIngress as u8;
    let (value, from, to, source) = if outbound {
        (tuple.source, config.internal_v6, config.external_v6, true)
    } else if inbound {
        (
            tuple.destination,
            config.external_v6,
            config.internal_v6,
            false,
        )
    } else {
        return ActionCode::Pass;
    };
    if !prefix_matches(value, from, config.prefix_len) {
        return ActionCode::Pass;
    }
    let translated = checksum_neutral_prefix(value, to, config.prefix_len);
    let l3 = descriptor.l3_offset as usize;
    let offset = if source { l3 + 8 } else { l3 + 24 };
    packet[offset..offset + 16].copy_from_slice(&translated);
    ActionCode::Rewrite
}

fn checksum_neutral_prefix(value: [u8; 16], prefix: [u8; 16], length: u8) -> [u8; 16] {
    let target_sum = address_sum(value);
    let mut translated = value;
    copy_prefix(&mut translated, prefix, length);
    translated[14] = 0;
    translated[15] = 0;
    let other_sum = address_sum(translated);
    let adjustment = ones_add(target_sum, !other_sum);
    translated[14..16].copy_from_slice(&adjustment.to_be_bytes());
    translated
}

fn address_sum(value: [u8; 16]) -> u16 {
    value.chunks_exact(2).fold(0u16, |sum, word| {
        ones_add(sum, u16::from_be_bytes([word[0], word[1]]))
    })
}

fn ones_add(left: u16, right: u16) -> u16 {
    let sum = u32::from(left) + u32::from(right);
    (sum as u16).wrapping_add((sum >> 16) as u16)
}

fn rewrite_v4_address(
    packet: &mut [u8],
    descriptor: PacketDescriptor,
    source: bool,
    old: [u8; 4],
    new: [u8; 4],
) {
    update_transport_address_checksum(packet, descriptor, extend(old), extend(new));
    rewrite_v4_header_address(packet, descriptor, source, old, new);
}

fn rewrite_v4_header_address(
    packet: &mut [u8],
    descriptor: PacketDescriptor,
    source: bool,
    old: [u8; 4],
    new: [u8; 4],
) {
    let l3 = descriptor.l3_offset as usize;
    let offset = if source { l3 + 12 } else { l3 + 16 };
    replace_address(&mut packet[l3 + 10..l3 + 12], &old, &new);
    packet[offset..offset + 4].copy_from_slice(&new);
}

fn rewrite_port(
    packet: &mut [u8],
    descriptor: PacketDescriptor,
    protocol: u8,
    source: bool,
    old: u16,
    new: u16,
) {
    let l4 = descriptor.l4_offset as usize;
    let offset = if matches!(protocol, 1 | 58) {
        l4 + 4
    } else if source {
        l4
    } else {
        l4 + 2
    };
    if let Some(checksum) = transport_checksum(packet, descriptor) {
        replace_checksum_word(checksum, old, new);
    }
    packet[offset..offset + 2].copy_from_slice(&new.to_be_bytes());
}

fn update_transport_address_checksum(
    packet: &mut [u8],
    descriptor: PacketDescriptor,
    old: [u8; 16],
    new: [u8; 16],
) {
    if descriptor.protocol == 1 {
        return;
    }
    if let Some(checksum) = transport_checksum(packet, descriptor) {
        let bytes = if descriptor.ip_version == 4 { 4 } else { 16 };
        replace_address(checksum, &old[..bytes], &new[..bytes]);
    }
}

fn transport_checksum(packet: &mut [u8], descriptor: PacketDescriptor) -> Option<&mut [u8]> {
    let l4 = descriptor.l4_offset as usize;
    let offset = match descriptor.protocol {
        6 => l4 + 16,
        17 => l4 + 6,
        1 | 58 => l4 + 2,
        _ => return None,
    };
    let checksum = packet.get_mut(offset..offset + 2)?;
    if descriptor.ip_version == 4 && descriptor.protocol == 17 && checksum == [0, 0] {
        None
    } else {
        Some(checksum)
    }
}

fn extend(value: [u8; 4]) -> [u8; 16] {
    let mut result = [0; 16];
    result[..4].copy_from_slice(&value);
    result
}

fn prefix_matches(value: [u8; 16], prefix: [u8; 16], length: u8) -> bool {
    let bytes = usize::from(length / 8);
    let bits = length % 8;
    value[..bytes] == prefix[..bytes]
        && (bits == 0
            || (value[bytes] & (0xff << (8 - bits))) == (prefix[bytes] & (0xff << (8 - bits))))
}

fn copy_prefix(value: &mut [u8; 16], prefix: [u8; 16], length: u8) {
    let bytes = usize::from(length / 8);
    let bits = length % 8;
    value[..bytes].copy_from_slice(&prefix[..bytes]);
    if bits != 0 {
        let mask = 0xff << (8 - bits);
        value[bytes] = (value[bytes] & !mask) | (prefix[bytes] & mask);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    loop {}
}
