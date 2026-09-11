use bexos_debug_wire::{Frame, METHOD_LIST_PROCESSES, decode_debug_status};
use kernel_fidl::{
    DebugProcessState, EncodeResult, FidlEncode, FidlTransport, FidlWireError, HandleRef,
    KernelDebugControlListProcessesResponse, KernelDebugControlPublicClient,
    KernelProcessDebugInfo, Status, WireVector,
};

struct ProcessTransport {
    fail: bool,
    invalid_length: bool,
    maximum_metadata: bool,
}
impl FidlTransport for ProcessTransport {
    fn call(
        &mut self,
        _: u64,
        _: &[u8],
        _: &[HandleRef],
        bytes: &mut [u8],
        handles: &mut [HandleRef],
    ) -> Result<EncodeResult, FidlWireError> {
        if self.fail {
            return Err(FidlWireError::Transport);
        }
        let mut name = [0; 64];
        name[..4].copy_from_slice(b"teed");
        let mut package_id = [0; 96];
        package_id[..18].copy_from_slice(b"bexos.service.teed");
        let records: Vec<_> = (1..=256)
            .map(|pid| KernelProcessDebugInfo {
                pid: if self.maximum_metadata {
                    u64::MAX - pid
                } else {
                    pid
                },
                main_thread_id: if self.maximum_metadata {
                    u64::MAX - pid
                } else {
                    pid
                },
                resource_group_id: if self.maximum_metadata {
                    u32::MAX - u32::from(pid == 256)
                } else {
                    1
                },
                resource_group_name_len: if self.maximum_metadata { 24 } else { 6 },
                resource_group_name: *b"system\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
                parent_resource_group_id: if self.maximum_metadata && pid != 256 {
                    u32::MAX - 1
                } else {
                    0
                },
                state: DebugProcessState::Running,
                name_len: if self.invalid_length {
                    65
                } else if self.maximum_metadata {
                    64
                } else {
                    4
                },
                name,
                package_id_len: if self.maximum_metadata { 96 } else { 18 },
                package_id,
            })
            .collect();
        KernelDebugControlListProcessesResponse {
            status: Status::Ok,
            processes: WireVector::from_slice(&records),
        }
        .encode(bytes, handles)
    }
}

#[test]
fn process_list_retains_kernel_identity_and_supports_large_lists() {
    let mut client = KernelDebugControlPublicClient::new(ProcessTransport {
        fail: false,
        invalid_length: false,
        maximum_metadata: false,
    });
    let processes = bexos_debugd::processes::list(&mut client).unwrap();
    assert_eq!(processes.len(), 256);
    assert_eq!(processes[0].name, "teed");
    assert_eq!(processes[0].package_id, "bexos.service.teed");
    assert_eq!(processes[0].state, "Running");
    assert_eq!(processes[0].resource_group_name, "system");
    assert_eq!(processes[0].resource_group_id, Some(1));
    assert_eq!(processes[0].main_thread_id, 1);
}

#[test]
fn process_query_failure_returns_error_instead_of_fabricated_process() {
    let mut client = KernelDebugControlPublicClient::new(ProcessTransport {
        fail: true,
        invalid_length: false,
        maximum_metadata: false,
    });
    let request = Frame {
        flags: 0,
        request_id: 7,
        method_id: METHOD_LIST_PROCESSES,
        payload: Vec::new(),
    };
    let response = bexos_debugd::processes::response(&request, &mut client);
    let status = decode_debug_status(&response.payload).unwrap();
    assert_ne!(status.status, 0);
    assert!(status.message.contains("transport failed"));
}

#[test]
fn malformed_kernel_identity_is_rejected() {
    let mut client = KernelDebugControlPublicClient::new(ProcessTransport {
        fail: false,
        invalid_length: true,
        maximum_metadata: false,
    });
    assert!(bexos_debugd::processes::list(&mut client).is_err());
}

#[test]
fn oversized_process_metadata_returns_an_encodable_error() {
    let mut client = KernelDebugControlPublicClient::new(ProcessTransport {
        fail: false,
        invalid_length: false,
        maximum_metadata: true,
    });
    let request = Frame {
        flags: 0,
        request_id: 7,
        method_id: METHOD_LIST_PROCESSES,
        payload: Vec::new(),
    };
    let response = bexos_debugd::processes::response(&request, &mut client);
    let status = decode_debug_status(&response.payload).unwrap();
    assert_ne!(status.status, 0);
    assert!(status.message.contains("64 KiB"));
    response.encode(&mut Vec::new()).unwrap();
}
