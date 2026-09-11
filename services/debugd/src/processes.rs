//! Kernel-owned process identities for the debug protocol.
use alloc::{vec, vec::Vec};
use bexos_debug_wire::{
    DebugStatusResponse, Frame, ProcessInfo, encode_debug_status, encode_process_list,
};
use kernel_fidl::{
    FidlTransport, HandleRef, KernelDebugControlListProcessesRequest,
    KernelDebugControlPublicClient, Status,
};

pub fn list<T: FidlTransport>(
    client: &mut KernelDebugControlPublicClient<T>,
) -> Result<Vec<ProcessInfo>, &'static str> {
    let mut request_bytes = [0; 16];
    // The protocol allows 256 bounded identity records, which exceed 4 KiB.
    let mut response_bytes = vec![0; 65536];
    let mut request_handles = [HandleRef { raw: 0 }; 1];
    let mut response_handles = [HandleRef { raw: 0 }; 1];
    let response = client
        .list_processes(
            &KernelDebugControlListProcessesRequest {},
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )
        .map_err(|_| "kernel debug FIDL transport failed")?;
    if response.status != Status::Ok {
        return Err("kernel debug FIDL denied process list");
    }
    let mut processes = Vec::new();
    for index in 0..response.processes.len() {
        let process = response
            .processes
            .get(index)
            .map_err(|_| "kernel process decode failed")?;
        let name = decode_name(&process.name, process.name_len)?;
        let package_id = decode_name(&process.package_id, process.package_id_len)?;
        processes.push(ProcessInfo {
            pid: process.pid,
            main_thread_id: process.main_thread_id,
            resource_group_id: Some(process.resource_group_id),
            resource_group_name: decode_name(
                &process.resource_group_name,
                process.resource_group_name_len,
            )?
            .into(),
            parent_resource_group_id: (process.parent_resource_group_id != 0)
                .then_some(process.parent_resource_group_id),
            parent_resource_group_name: alloc::string::String::new(),
            name: name.into(),
            state: alloc::format!("{:?}", process.state),
            package_id: package_id.into(),
        });
    }
    // Resolve parent display names from this same kernel snapshot, without inventing names.
    let groups: Vec<_> = processes
        .iter()
        .filter_map(|p| {
            p.resource_group_id
                .map(|id| (id, p.resource_group_name.clone()))
        })
        .collect();
    for process in &mut processes {
        if let Some(id) = process.parent_resource_group_id {
            if let Some((_, name)) = groups.iter().find(|(group, _)| *group == id) {
                process.parent_resource_group_name = name.clone();
            }
        }
    }
    Ok(processes)
}

fn decode_name(bytes: &[u8], len: u32) -> Result<&str, &'static str> {
    let bytes = bytes
        .get(..len as usize)
        .ok_or("invalid kernel process identity length")?;
    core::str::from_utf8(bytes).map_err(|_| "invalid kernel process identity UTF-8")
}

pub fn response<T: FidlTransport>(
    request: &Frame,
    client: &mut KernelDebugControlPublicClient<T>,
) -> Frame {
    let mut payload = Vec::new();
    match list(client) {
        Ok(processes) => encode_process_list(&processes, &mut payload),
        Err(message) => encode_debug_status(
            &DebugStatusResponse {
                status: -1,
                message: message.into(),
            },
            &mut payload,
        ),
    }
    // Names and IDs are individually bounded, but their largest combination
    // can exceed a BXD1 frame. Return an error rather than silently dropping
    // the response when the transport cannot encode it.
    if payload.len() > bexos_debug_wire::MAX_PAYLOAD_LEN {
        encode_debug_status(
            &DebugStatusResponse {
                status: -1,
                message: "process metadata exceeds the 64 KiB debug response limit".into(),
            },
            &mut payload,
        );
    }
    Frame {
        flags: request.flags,
        request_id: request.request_id,
        method_id: request.method_id,
        payload,
    }
}

/// The legacy diagnostic uses the same live kernel snapshot as the typed API.
pub fn exec_response<T: FidlTransport>(
    request: &Frame,
    client: &mut KernelDebugControlPublicClient<T>,
) -> Frame {
    let response = match list(client) {
        Ok(processes) => bexos_debug_wire::ExecResponse {
            exit_code: 0,
            stdout: processes
                .iter()
                .map(|p| {
                    alloc::format!(
                        "{}\t{}\t{}\t{}\t{} ({})\n",
                        p.pid,
                        p.state,
                        p.name,
                        p.package_id,
                        p.resource_group_name,
                        p.resource_group_id
                            .map_or_else(|| "unknown".into(), |id| alloc::format!("{id}"))
                    )
                })
                .collect(),
            stderr: alloc::string::String::new(),
        },
        Err(e) => bexos_debug_wire::ExecResponse {
            exit_code: 1,
            stdout: alloc::string::String::new(),
            stderr: alloc::format!("{e}\n"),
        },
    };
    let mut payload = Vec::new();
    bexos_debug_wire::encode_exec_response(&response, &mut payload);
    Frame {
        flags: request.flags,
        request_id: request.request_id,
        method_id: request.method_id,
        payload,
    }
}
