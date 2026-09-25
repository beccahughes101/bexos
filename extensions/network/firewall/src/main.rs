#![no_std]
#![no_main]

use bexos_network_extension_abi::{
    ABI_VERSION, ActionCode, Direction, MAX_FIREWALL_RULES, MAX_TRACKED_FLOWS, PacketAction,
};
use bexos_network_extension_guest::{Tuple, VECTOR_BUFFER_CAPACITY, VectorView, fragment, tuple};
use core::panic::PanicInfo;

const RULE_BYTES: usize = 56;
const TCP_TIMEOUT_NS: u64 = 300_000_000_000;
const UDP_TIMEOUT_NS: u64 = 60_000_000_000;
const ICMP_TIMEOUT_NS: u64 = 30_000_000_000;
const MAX_FRAGMENT_DECISIONS: usize = 256;

#[link(wasm_import_module = "bexos:net/extension@1.0.0")]
unsafe extern "C" {
    #[link_name = "monotonic-ns"]
    fn monotonic_ns() -> u64;
}

#[derive(Clone, Copy)]
struct Rule {
    action: u8,
    direction: u8,
    protocol: u8,
    state: u8,
    source_zone: u16,
    destination_zone: u16,
    source_prefix: u8,
    destination_prefix: u8,
    source: [u8; 16],
    destination: [u8; 16],
    source_port_start: u16,
    source_port_end: u16,
    destination_port_start: u16,
    destination_port_end: u16,
    icmp_type: u8,
    icmp_code: u8,
}

const EMPTY_RULE: Rule = Rule {
    action: 0,
    direction: 0,
    protocol: 0,
    state: 0,
    source_zone: 0,
    destination_zone: 0,
    source_prefix: 0,
    destination_prefix: 0,
    source: [0; 16],
    destination: [0; 16],
    source_port_start: 0,
    source_port_end: u16::MAX,
    destination_port_start: 0,
    destination_port_end: u16::MAX,
    icmp_type: u8::MAX,
    icmp_code: u8::MAX,
};

#[derive(Clone, Copy)]
struct Flow {
    used: bool,
    version: u8,
    protocol: u8,
    source: [u8; 16],
    destination: [u8; 16],
    source_port: u16,
    destination_port: u16,
    expires_ns: u64,
}

const EMPTY_FLOW: Flow = Flow {
    used: false,
    version: 0,
    protocol: 0,
    source: [0; 16],
    destination: [0; 16],
    source_port: 0,
    destination_port: 0,
    expires_ns: 0,
};

#[derive(Clone, Copy)]
struct FragmentDecision {
    used: bool,
    version: u8,
    protocol: u8,
    direction: u8,
    allowed: bool,
    source: [u8; 16],
    destination: [u8; 16],
    id: u32,
    expires_ns: u64,
}

const EMPTY_FRAGMENT: FragmentDecision = FragmentDecision {
    used: false,
    version: 0,
    protocol: 0,
    direction: 0,
    allowed: false,
    source: [0; 16],
    destination: [0; 16],
    id: 0,
    expires_ns: 0,
};

#[repr(C)]
struct Checkpoint {
    high_water: u32,
    fragment_high_water: u32,
    rule_counters: [u64; MAX_FIREWALL_RULES + 1],
    flows: [Flow; MAX_TRACKED_FLOWS],
    fragments: [FragmentDecision; MAX_FRAGMENT_DECISIONS],
}

static mut BUFFER: core::mem::MaybeUninit<[u8; VECTOR_BUFFER_CAPACITY]> =
    core::mem::MaybeUninit::uninit();
static mut RULES: [Rule; MAX_FIREWALL_RULES] = [EMPTY_RULE; MAX_FIREWALL_RULES];
static mut RULE_COUNT: usize = 0;
static mut CHECKPOINT: Checkpoint = Checkpoint {
    high_water: 0,
    fragment_high_water: 0,
    rule_counters: [0; MAX_FIREWALL_RULES + 1],
    flows: [EMPTY_FLOW; MAX_TRACKED_FLOWS],
    fragments: [EMPTY_FRAGMENT; MAX_FRAGMENT_DECISIONS],
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
    if pointer < 0 || length < 8 || length as usize > 8 + MAX_FIREWALL_RULES * RULE_BYTES {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length as usize) };
    if &bytes[..4] != b"NFW1" {
        return -1;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if count > MAX_FIREWALL_RULES || bytes.len() != 8 + count * RULE_BYTES {
        return -1;
    }
    for index in 0..count {
        let start = 8 + index * RULE_BYTES;
        let rule = &bytes[start..start + RULE_BYTES];
        let parsed = Rule {
            action: rule[0],
            direction: rule[1],
            protocol: rule[2],
            state: rule[3],
            source_zone: u16::from_le_bytes([rule[4], rule[5]]),
            destination_zone: u16::from_le_bytes([rule[6], rule[7]]),
            source_prefix: rule[8],
            destination_prefix: rule[9],
            source: rule[12..28].try_into().unwrap(),
            destination: rule[28..44].try_into().unwrap(),
            source_port_start: u16::from_le_bytes([rule[44], rule[45]]),
            source_port_end: u16::from_le_bytes([rule[46], rule[47]]),
            destination_port_start: u16::from_le_bytes([rule[48], rule[49]]),
            destination_port_end: u16::from_le_bytes([rule[50], rule[51]]),
            icmp_type: rule[52],
            icmp_code: rule[53],
        };
        if parsed.action > 1
            || parsed.direction > Direction::VirtualIngress as u8
            || parsed.source_prefix > 128
            || parsed.destination_prefix > 128
            || parsed.source_port_start > parsed.source_port_end
            || parsed.destination_port_start > parsed.destination_port_end
        {
            return -1;
        }
        unsafe { core::ptr::addr_of_mut!(RULES[index]).write(parsed) };
    }
    unsafe { RULE_COUNT = count };
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
    let now = unsafe { monotonic_ns() };
    for index in 0..vector.count() {
        let Some(descriptor) = (unsafe { vector.descriptor(index) }) else {
            return -1;
        };
        let action = if let Some(packet) = unsafe { vector.packet_mut(descriptor) } {
            decide(packet, descriptor, now)
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

fn decide(
    packet: &[u8],
    descriptor: bexos_network_extension_abi::PacketDescriptor,
    now: u64,
) -> ActionCode {
    let Some(tuple) = tuple(packet, descriptor) else {
        return ActionCode::Drop;
    };
    let fragment = fragment(packet, descriptor);
    if let Some(fragment) = fragment.filter(|fragment| fragment.offset != 0) {
        return fragment_decision(tuple, descriptor.direction, fragment.id, now)
            .unwrap_or(ActionCode::Drop);
    }
    let decision = decide_tuple(packet, descriptor, tuple, now);
    if let Some(fragment) = fragment.filter(|fragment| fragment.more) {
        if !record_fragment(
            tuple,
            descriptor.direction,
            fragment.id,
            decision == ActionCode::Pass,
            now,
        ) && decision == ActionCode::Pass
        {
            return ActionCode::Drop;
        }
    }
    decision
}

fn decide_tuple(
    packet: &[u8],
    descriptor: bexos_network_extension_abi::PacketDescriptor,
    tuple: Tuple,
    now: u64,
) -> ActionCode {
    let established = find_reverse(tuple, now) || find_related(packet, descriptor, now);
    if tuple.protocol == 6
        && !established
        && descriptor.direction == Direction::VirtualIngress as u8
        && !valid_new_tcp(packet, descriptor)
    {
        return ActionCode::Drop;
    }
    let rule_count = unsafe { RULE_COUNT };
    for index in 0..rule_count {
        let rule = unsafe { core::ptr::addr_of!(RULES[index]).read() };
        if matches_rule(rule, tuple, packet, descriptor, established) {
            unsafe {
                CHECKPOINT.rule_counters[index] = CHECKPOINT.rule_counters[index].saturating_add(1);
            }
            if rule.action == 0 {
                return ActionCode::Drop;
            }
            if !established
                && descriptor.direction == Direction::VirtualIngress as u8
                && !track(tuple, now)
            {
                return ActionCode::Drop;
            }
            return ActionCode::Pass;
        }
    }
    unsafe {
        CHECKPOINT.rule_counters[MAX_FIREWALL_RULES] =
            CHECKPOINT.rule_counters[MAX_FIREWALL_RULES].saturating_add(1);
    }
    if established {
        refresh_reverse(tuple, now);
        ActionCode::Pass
    } else if descriptor.direction == Direction::VirtualIngress as u8 && track(tuple, now) {
        ActionCode::Pass
    } else {
        ActionCode::Drop
    }
}

fn valid_new_tcp(packet: &[u8], descriptor: bexos_network_extension_abi::PacketDescriptor) -> bool {
    let l4 = descriptor.l4_offset as usize;
    packet
        .get(l4 + 13)
        .is_some_and(|flags| flags & 0x02 != 0 && flags & (0x04 | 0x10) == 0)
}

fn fragment_decision(tuple: Tuple, direction: u8, id: u32, now: u64) -> Option<ActionCode> {
    let high = unsafe { CHECKPOINT.fragment_high_water as usize }.min(MAX_FRAGMENT_DECISIONS);
    (0..high).find_map(|index| {
        let entry = unsafe { core::ptr::addr_of!(CHECKPOINT.fragments[index]).read() };
        (entry.used
            && entry.expires_ns >= now
            && entry.version == tuple.version
            && entry.protocol == tuple.protocol
            && entry.direction == direction
            && entry.source == tuple.source
            && entry.destination == tuple.destination
            && entry.id == id)
            .then_some(if entry.allowed {
                ActionCode::Pass
            } else {
                ActionCode::Drop
            })
    })
}

fn record_fragment(tuple: Tuple, direction: u8, id: u32, allowed: bool, now: u64) -> bool {
    let high = unsafe { CHECKPOINT.fragment_high_water as usize }.min(MAX_FRAGMENT_DECISIONS);
    let index = (0..high)
        .find(|index| {
            let entry = unsafe { core::ptr::addr_of!(CHECKPOINT.fragments[*index]).read() };
            !entry.used || entry.expires_ns < now
        })
        .or_else(|| (high < MAX_FRAGMENT_DECISIONS).then_some(high));
    let Some(index) = index else { return false };
    unsafe {
        core::ptr::addr_of_mut!(CHECKPOINT.fragments[index]).write(FragmentDecision {
            used: true,
            version: tuple.version,
            protocol: tuple.protocol,
            direction,
            allowed,
            source: tuple.source,
            destination: tuple.destination,
            id,
            expires_ns: now.saturating_add(ICMP_TIMEOUT_NS),
        });
        if index == high {
            CHECKPOINT.fragment_high_water = (high + 1) as u32;
        }
    }
    true
}

fn matches_rule(
    rule: Rule,
    tuple: Tuple,
    packet: &[u8],
    descriptor: bexos_network_extension_abi::PacketDescriptor,
    established: bool,
) -> bool {
    let (icmp_type, icmp_code) = if matches!(tuple.protocol, 1 | 58) {
        let l4 = descriptor.l4_offset as usize;
        packet
            .get(l4..l4 + 2)
            .map_or((u8::MAX, u8::MAX), |bytes| (bytes[0], bytes[1]))
    } else {
        (u8::MAX, u8::MAX)
    };
    (rule.direction == 0 || rule.direction == descriptor.direction)
        && (rule.protocol == 0 || rule.protocol == tuple.protocol)
        && (rule.state == 0
            || (rule.state == 1 && !established)
            || (rule.state == 2 && established))
        && (rule.source_zone == 0 || rule.source_zone == descriptor.source_zone)
        && (rule.destination_zone == 0 || rule.destination_zone == descriptor.destination_zone)
        && prefix_matches(tuple.source, rule.source, rule.source_prefix)
        && prefix_matches(tuple.destination, rule.destination, rule.destination_prefix)
        && (rule.source_port_start..=rule.source_port_end).contains(&tuple.source_port)
        && (rule.destination_port_start..=rule.destination_port_end)
            .contains(&tuple.destination_port)
        && (rule.icmp_type == u8::MAX || rule.icmp_type == icmp_type)
        && (rule.icmp_code == u8::MAX || rule.icmp_code == icmp_code)
}

fn prefix_matches(value: [u8; 16], expected: [u8; 16], prefix: u8) -> bool {
    let bytes = usize::from(prefix / 8);
    let bits = prefix % 8;
    value[..bytes] == expected[..bytes]
        && (bits == 0
            || (value[bytes] & (0xff << (8 - bits))) == (expected[bytes] & (0xff << (8 - bits))))
}

fn find_reverse(tuple: Tuple, now: u64) -> bool {
    let high_water = unsafe { CHECKPOINT.high_water as usize }.min(MAX_TRACKED_FLOWS);
    for index in 0..high_water {
        let flow = unsafe { core::ptr::addr_of!(CHECKPOINT.flows[index]).read() };
        if flow.used
            && flow.expires_ns >= now
            && flow.version == tuple.version
            && flow.protocol == tuple.protocol
            && flow.source == tuple.destination
            && flow.destination == tuple.source
            && flow.source_port == tuple.destination_port
            && flow.destination_port == tuple.source_port
        {
            return true;
        }
    }
    false
}

fn find_related(
    packet: &[u8],
    descriptor: bexos_network_extension_abi::PacketDescriptor,
    now: u64,
) -> bool {
    if !matches!(descriptor.protocol, 1 | 58) {
        return false;
    }
    let l4 = descriptor.l4_offset as usize;
    let Some(kind) = packet.get(l4).copied() else {
        return false;
    };
    let is_error = (descriptor.protocol == 1 && matches!(kind, 3 | 11 | 12))
        || (descriptor.protocol == 58 && matches!(kind, 1..=4));
    if !is_error {
        return false;
    }
    let quoted = l4 + 8;
    let Some(version) = packet.get(quoted).map(|byte| byte >> 4) else {
        return false;
    };
    let mut source = [0u8; 16];
    let mut destination = [0u8; 16];
    let (protocol, transport) = match version {
        4 if packet.len() >= quoted + 20 => {
            let header = usize::from(packet[quoted] & 0x0f) * 4;
            if header < 20 || packet.len() < quoted + header + 4 {
                return false;
            }
            source[..4].copy_from_slice(&packet[quoted + 12..quoted + 16]);
            destination[..4].copy_from_slice(&packet[quoted + 16..quoted + 20]);
            (packet[quoted + 9], quoted + header)
        }
        6 if packet.len() >= quoted + 44 => {
            source.copy_from_slice(&packet[quoted + 8..quoted + 24]);
            destination.copy_from_slice(&packet[quoted + 24..quoted + 40]);
            (packet[quoted + 6], quoted + 40)
        }
        _ => return false,
    };
    let (source_port, destination_port) = if matches!(protocol, 6 | 17) {
        (
            u16::from_be_bytes([packet[transport], packet[transport + 1]]),
            u16::from_be_bytes([packet[transport + 2], packet[transport + 3]]),
        )
    } else if matches!(protocol, 1 | 58) && packet.len() >= transport + 6 {
        (
            u16::from_be_bytes([packet[transport + 4], packet[transport + 5]]),
            0,
        )
    } else {
        (0, 0)
    };
    let high_water = unsafe { CHECKPOINT.high_water as usize }.min(MAX_TRACKED_FLOWS);
    (0..high_water).any(|index| {
        let flow = unsafe { core::ptr::addr_of!(CHECKPOINT.flows[index]).read() };
        flow.used
            && flow.expires_ns >= now
            && flow.version == version
            && flow.protocol == protocol
            && flow.source == source
            && flow.destination == destination
            && flow.source_port == source_port
            && flow.destination_port == destination_port
    })
}

fn refresh_reverse(tuple: Tuple, now: u64) {
    let high_water = unsafe { CHECKPOINT.high_water as usize }.min(MAX_TRACKED_FLOWS);
    for index in 0..high_water {
        let mut flow = unsafe { core::ptr::addr_of!(CHECKPOINT.flows[index]).read() };
        if flow.used
            && flow.version == tuple.version
            && flow.protocol == tuple.protocol
            && flow.source == tuple.destination
            && flow.destination == tuple.source
            && flow.source_port == tuple.destination_port
            && flow.destination_port == tuple.source_port
        {
            flow.expires_ns = now.saturating_add(timeout(tuple.protocol));
            unsafe { core::ptr::addr_of_mut!(CHECKPOINT.flows[index]).write(flow) };
            return;
        }
    }
}

fn track(tuple: Tuple, now: u64) -> bool {
    let high_water = unsafe { CHECKPOINT.high_water as usize }.min(MAX_TRACKED_FLOWS);
    for index in 0..high_water {
        let flow = unsafe { core::ptr::addr_of!(CHECKPOINT.flows[index]).read() };
        if !flow.used || flow.expires_ns < now {
            unsafe {
                core::ptr::addr_of_mut!(CHECKPOINT.flows[index]).write(Flow {
                    used: true,
                    version: tuple.version,
                    protocol: tuple.protocol,
                    source: tuple.source,
                    destination: tuple.destination,
                    source_port: tuple.source_port,
                    destination_port: tuple.destination_port,
                    expires_ns: now.saturating_add(timeout(tuple.protocol)),
                })
            };
            return true;
        }
    }
    if high_water == MAX_TRACKED_FLOWS {
        return false;
    }
    unsafe {
        core::ptr::addr_of_mut!(CHECKPOINT.flows[high_water]).write(Flow {
            used: true,
            version: tuple.version,
            protocol: tuple.protocol,
            source: tuple.source,
            destination: tuple.destination,
            source_port: tuple.source_port,
            destination_port: tuple.destination_port,
            expires_ns: now.saturating_add(timeout(tuple.protocol)),
        });
        CHECKPOINT.high_water = (high_water + 1) as u32;
    }
    true
}

fn timeout(protocol: u8) -> u64 {
    match protocol {
        6 => TCP_TIMEOUT_NS,
        17 => UDP_TIMEOUT_NS,
        _ => ICMP_TIMEOUT_NS,
    }
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    loop {}
}
