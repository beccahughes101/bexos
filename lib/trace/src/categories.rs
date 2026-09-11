use alloc::string::String;

pub const CATEGORY_KERNEL_SCHED: u32 = 0x0001;
pub const CATEGORY_IPC_MESSAGES: u32 = 0x0002;
pub const CATEGORY_VFS_IO: u32 = 0x0004;
pub const CATEGORY_NETWORK_STACK: u32 = 0x0008;
pub const CATEGORY_UI_FRAMES: u32 = 0x0010;
pub const CATEGORY_APP_CUSTOM: u32 = 0x0020;
pub const CATEGORY_DEBUG_SERVICE: u32 = 0x0040;
pub const CATEGORY_ALL: u32 = CATEGORY_KERNEL_SCHED
    | CATEGORY_IPC_MESSAGES
    | CATEGORY_VFS_IO
    | CATEGORY_NETWORK_STACK
    | CATEGORY_UI_FRAMES
    | CATEGORY_APP_CUSTOM
    | CATEGORY_DEBUG_SERVICE;

pub fn category_name(category: u32) -> &'static str {
    match category {
        CATEGORY_KERNEL_SCHED => "kernel_sched",
        CATEGORY_IPC_MESSAGES => "ipc_messages",
        CATEGORY_VFS_IO => "vfs_io",
        CATEGORY_NETWORK_STACK => "network_stack",
        CATEGORY_UI_FRAMES => "ui_frames",
        CATEGORY_APP_CUSTOM => "app_custom",
        CATEGORY_DEBUG_SERVICE => "debug_service",
        _ => "unknown",
    }
}

pub fn parse_category_list(input: &str) -> Option<u32> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
        return Some(CATEGORY_ALL);
    }
    let mut categories = 0;
    for token in trimmed.split(',') {
        let category = match token.trim().to_ascii_lowercase().as_str() {
            "kernel" | "kernel_sched" | "sched" => CATEGORY_KERNEL_SCHED,
            "ipc" | "ipc_messages" => CATEGORY_IPC_MESSAGES,
            "vfs" | "vfs_io" => CATEGORY_VFS_IO,
            "net" | "network" | "network_stack" => CATEGORY_NETWORK_STACK,
            "ui" | "ui_frames" => CATEGORY_UI_FRAMES,
            "app" | "app_custom" => CATEGORY_APP_CUSTOM,
            "debug" | "debug_service" | "service" => CATEGORY_DEBUG_SERVICE,
            _ => return None,
        };
        categories |= category;
    }
    Some(categories)
}

pub fn category_list(categories: u32) -> String {
    let mut out = String::new();
    for (bit, name) in [
        (CATEGORY_KERNEL_SCHED, "kernel_sched"),
        (CATEGORY_IPC_MESSAGES, "ipc_messages"),
        (CATEGORY_VFS_IO, "vfs_io"),
        (CATEGORY_NETWORK_STACK, "network_stack"),
        (CATEGORY_UI_FRAMES, "ui_frames"),
        (CATEGORY_APP_CUSTOM, "app_custom"),
        (CATEGORY_DEBUG_SERVICE, "debug_service"),
    ] {
        if categories & bit != 0 {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(name);
        }
    }
    out
}
