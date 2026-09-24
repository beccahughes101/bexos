//! Resolve numeric syscall protocol ordinals across every capability of that protocol.
use kernel_fidl::*;

pub fn syscall_method(protocol: u64, ordinal: u64) -> Option<&'static str> {
    let methods = match protocol {
        1 => CHANNEL_CONTROL_PUBLIC_METHODS,
        10 => SOCKET_CONTROL_PUBLIC_METHODS,
        2 => VIRTUAL_MEMORY_PUBLIC_METHODS,
        3 => TASK_CONTROL_PUBLIC_METHODS,
        4 => SYSTEM_PRIVILEGED_BEXOS_SYSTEM_PRIVILEGED_METHODS,
        5 => OBJECT_CONTROL_PUBLIC_METHODS,
        6 => KERNEL_DEBUG_CONTROL_PUBLIC_METHODS,
        12 => KERNEL_TRACE_CONTROL_BEXOS_SYSTEM_PRIVILEGED_METHODS,
        9 => SECURE_MONITOR_PUBLIC_METHODS,
        8 => CLOCK_PUBLIC_METHODS,
        13 => RANDOM_PUBLIC_METHODS,
        14 => RESTRICTED_PUBLIC_METHODS,
        11 => PROFILE_PROVIDER_PUBLIC_METHODS,
        _ => return None,
    };
    methods
        .iter()
        .chain(if protocol == 4 {
            SYSTEM_PRIVILEGED_SET_TIME_METHODS
        } else {
            &[]
        })
        .find(|m| m.ordinal == ordinal)
        .map(|m| m.name)
}
