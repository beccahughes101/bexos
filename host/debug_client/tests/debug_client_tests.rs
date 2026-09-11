use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::thread;

mod update_upload;

use bexos_debug_client::{DebugClient, UnixSocketTransport};
use bexos_debug_wire::{
    AppInfo, DebugStatusResponse, Frame, HealthCheckResponse, METHOD_CHECK_UPDATES,
    METHOD_GET_COMPONENT_CONFIG, METHOD_HEALTH_CHECK, METHOD_LAUNCH_APP, METHOD_LIST_APPS,
    METHOD_LIST_PROCESSES, METHOD_RESET_COMPONENT_CONFIG, METHOD_SET_COMPONENT_CONFIG,
    METHOD_TRACE_START, METHOD_TRACE_STATUS, METHOD_TRACE_STOP, METHOD_UNINSTALL_APP, ProcessInfo,
    TraceStatusResponse, TraceStopResponse, UpdateCandidateInfo, UpdateCheckResponse, UserInfo,
    decode_app_launch, decode_app_uninstall, decode_component_config_reset,
    decode_component_config_set, decode_trace_start_request, decode_trace_stop_request,
    encode_app_list, encode_component_config_get_response,
    encode_component_config_mutation_response, encode_debug_status, encode_health_response,
    encode_process_list, encode_trace_status_response, encode_trace_stop_response,
    encode_update_check_response, encode_user_list_response, parse_frame,
};

#[test]
fn batch_queues_all_requests_and_reorders_responses() {
    use bexos_debug_client::DebugTransport;
    struct Transport {
        response: Vec<u8>,
        writes: usize,
    }
    impl DebugTransport for Transport {
        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.writes += 1;
            assert_eq!(self.writes, 1);
            let (first, used) = parse_frame(bytes).unwrap();
            let (second, used2) = parse_frame(&bytes[used..]).unwrap();
            assert_eq!(used + used2, bytes.len());
            second.encode(&mut self.response).unwrap();
            first.encode(&mut self.response).unwrap();
            Ok(())
        }
        fn read_chunk(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let n = bytes.len().min(self.response.len());
            bytes[..n].copy_from_slice(&self.response[..n]);
            self.response.drain(..n);
            Ok(n)
        }
    }
    let mut client = DebugClient::new(Transport {
        response: Vec::new(),
        writes: 0,
    });
    let responses = client
        .call_batch(&[(METHOD_HEALTH_CHECK, vec![1]), (METHOD_LIST_APPS, vec![2])])
        .unwrap();
    assert_eq!(responses[0].payload, [1]);
    assert_eq!(responses[1].payload, [2]);
}

#[test]
fn client_skips_serial_noise_and_decodes_health() {
    let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut request = [0u8; 128];
        let n = server_stream.read(&mut request).unwrap();
        let (frame, _) = parse_frame(&request[..n]).unwrap();
        assert_eq!(frame.method_id, METHOD_HEALTH_CHECK);
        let mut payload = Vec::new();
        encode_health_response(
            &HealthCheckResponse {
                service_name: "debugd".into(),
                status: "SERVING".into(),
                version: "qemu-socket-v1".into(),
            },
            &mut payload,
        );
        server_stream.write_all(b"debugd: ready\n").unwrap();
        let mut response = Vec::new();
        Frame {
            flags: 0,
            request_id: frame.request_id,
            method_id: frame.method_id,
            payload,
        }
        .encode(&mut response)
        .unwrap();
        server_stream.write_all(&response).unwrap();
    });

    let mut client = DebugClient::new(UnixSocketTransport::from_stream(client_stream));
    let health = client.health_check().unwrap();
    assert_eq!(health.service_name, "debugd");
    assert_eq!(health.status, "SERVING");
    assert!(
        client
            .received_trace()
            .windows(b"debugd: ready\n".len())
            .any(|bytes| bytes == b"debugd: ready\n")
    );
    server.join().unwrap();
}

#[test]
fn client_lists_processes() {
    let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut request = [0u8; 128];
        let n = server_stream.read(&mut request).unwrap();
        let (frame, _) = parse_frame(&request[..n]).unwrap();
        assert_eq!(frame.method_id, METHOD_LIST_PROCESSES);
        let mut payload = Vec::new();
        encode_process_list(
            &[ProcessInfo {
                pid: 1,
                name: "debugd".into(),
                state: "RUNNING".into(),
                package_id: "bexos.driver.debugd".into(),
                ..Default::default()
            }],
            &mut payload,
        );
        let mut response = b"serial text\n".to_vec();
        Frame {
            flags: 0,
            request_id: frame.request_id,
            method_id: frame.method_id,
            payload,
        }
        .encode(&mut response)
        .unwrap();
        server_stream.write_all(&response).unwrap();
    });

    let mut client = DebugClient::new(UnixSocketTransport::from_stream(client_stream));
    let processes = client.list_processes().unwrap();
    assert_eq!(processes[0].name, "debugd");
    server.join().unwrap();
}

#[test]
fn process_query_error_is_not_an_empty_successful_list() {
    let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut request = [0; 128];
        let n = server_stream.read(&mut request).unwrap();
        let (frame, _) = parse_frame(&request[..n]).unwrap();
        let mut payload = Vec::new();
        encode_debug_status(
            &DebugStatusResponse {
                status: -1,
                message: "kernel query failed".into(),
            },
            &mut payload,
        );
        let mut response = Vec::new();
        Frame { payload, ..frame }.encode(&mut response).unwrap();
        server_stream.write_all(&response).unwrap();
    });
    let mut client = DebugClient::new(UnixSocketTransport::from_stream(client_stream));
    assert!(
        matches!(client.list_processes(), Err(bexos_debug_client::DebugClientError::RemoteStatus(status)) if status.status == -1)
    );
    server.join().unwrap();
}

#[test]
fn client_checks_updates() {
    let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut request = [0u8; 256];
        let n = server_stream.read(&mut request).unwrap();
        let (frame, _) = parse_frame(&request[..n]).unwrap();
        assert_eq!(frame.method_id, METHOD_CHECK_UPDATES);
        let mut payload = Vec::new();
        encode_update_check_response(
            &UpdateCheckResponse {
                status: 0,
                message: "updates available".into(),
                candidates: vec![UpdateCandidateInfo {
                    target: "com.bexos.demo".into(),
                    path: "apps/demo.bex".into(),
                    url: "https://repo.example/apps/demo.bex".into(),
                    kind: 1,
                    generation: 4,
                    length: 17,
                }],
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);
    });

    let mut client = DebugClient::new(UnixSocketTransport::from_stream(client_stream));
    let response = client
        .check_updates(2, "com.bexos.demo", false, false, false)
        .unwrap();
    assert_eq!(response.candidates[0].target, "com.bexos.demo");
    server.join().unwrap();
}

#[test]
fn client_lists_and_controls_apps() {
    let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut temp = [0u8; 256];

        while buffer.len() < 20 {
            let n = server_stream.read(&mut temp).unwrap();
            buffer.extend_from_slice(&temp[..n]);
        }
        let (frame, used) = parse_frame(&buffer).unwrap();
        buffer.drain(..used);
        assert_eq!(frame.method_id, METHOD_LIST_APPS);
        let mut payload = Vec::new();
        encode_app_list(
            &[AppInfo {
                package_id: "bexos.platform.storage_verify".into(),
                name: "storage_verify".into(),
                state: "Running".into(),
                source: "SystemImage".into(),
                protected: true,
            }],
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);

        let (frame, used) = read_frame(&mut server_stream, &mut buffer);
        buffer.drain(..used);
        assert_eq!(frame.method_id, METHOD_UNINSTALL_APP);
        assert_eq!(
            decode_app_uninstall(&frame.payload).unwrap().package_id,
            "com.example:demo"
        );
        let mut payload = Vec::new();
        encode_debug_status(
            &DebugStatusResponse {
                status: 0,
                message: "ok".into(),
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);

        let (frame, _) = read_frame(&mut server_stream, &mut buffer);
        assert_eq!(frame.method_id, METHOD_LAUNCH_APP);
        let launch = decode_app_launch(&frame.payload).unwrap();
        assert_eq!(launch.package_id, "com.example:demo");
        assert_eq!(launch.process_name, "demo");
        let mut payload = Vec::new();
        encode_debug_status(
            &DebugStatusResponse {
                status: 0,
                message: "ok".into(),
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);
    });

    let mut client = DebugClient::new(UnixSocketTransport::from_stream(client_stream));
    let apps = client.list_apps().unwrap();
    assert_eq!(apps[0].package_id, "bexos.platform.storage_verify");
    client.uninstall_app("com.example:demo").unwrap();
    client.launch_app("com.example:demo", "demo", 0, 0).unwrap();
    server.join().unwrap();
}

#[test]
fn client_gets_sets_and_resets_component_config() {
    let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut buffer = Vec::new();
        let (frame, used) = read_frame(&mut server_stream, &mut buffer);
        buffer.drain(..used);
        assert_eq!(frame.method_id, METHOD_GET_COMPONENT_CONFIG);
        let mut payload = Vec::new();
        encode_component_config_get_response(
            &bexos_debug_wire::ComponentConfigGetResponse {
                status: 0,
                generation: 3,
                config: b"BEXCFG".to_vec(),
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);

        let (frame, used) = read_frame(&mut server_stream, &mut buffer);
        buffer.drain(..used);
        assert_eq!(frame.method_id, METHOD_SET_COMPONENT_CONFIG);
        let set = decode_component_config_set(&frame.payload).unwrap();
        assert_eq!(set.package_id, "bexos.service.netstackd");
        assert_eq!(set.expected_generation, 3);
        assert_eq!(set.config, b"BEXCFG".to_vec());
        let mut payload = Vec::new();
        encode_component_config_mutation_response(
            &bexos_debug_wire::ComponentConfigMutationResponse {
                status: 0,
                generation: 4,
                message: "component config updated".into(),
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);

        let (frame, _) = read_frame(&mut server_stream, &mut buffer);
        assert_eq!(frame.method_id, METHOD_RESET_COMPONENT_CONFIG);
        let reset = decode_component_config_reset(&frame.payload).unwrap();
        assert_eq!(reset.package_id, "bexos.service.netstackd");
        assert_eq!(reset.expected_generation, 4);
        let mut payload = Vec::new();
        encode_component_config_mutation_response(
            &bexos_debug_wire::ComponentConfigMutationResponse {
                status: 0,
                generation: 5,
                message: "component config reset".into(),
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);
    });

    let mut client = DebugClient::new(UnixSocketTransport::from_stream(client_stream));
    let current = client
        .get_component_config("bexos.service.netstackd")
        .unwrap();
    assert_eq!(current.generation, 3);
    assert_eq!(current.config, b"BEXCFG".to_vec());
    let updated = client
        .set_component_config("bexos.service.netstackd", 3, b"BEXCFG")
        .unwrap();
    assert_eq!(updated.generation, 4);
    let reset = client
        .reset_component_config("bexos.service.netstackd", 4)
        .unwrap();
    assert_eq!(reset.generation, 5);
    server.join().unwrap();
}

#[test]
fn client_lists_users() {
    let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut request = [0u8; 128];
        let n = server_stream.read(&mut request).unwrap();
        let (frame, _) = parse_frame(&request[..n]).unwrap();
        assert_eq!(frame.method_id, bexos_debug_wire::METHOD_LIST_USERS);
        let mut payload = Vec::new();
        encode_user_list_response(
            &bexos_debug_wire::UserListResponse {
                status: 0,
                users: vec![UserInfo {
                    uid: 1000,
                    name: "alice".into(),
                    display_name: "Alice".into(),
                    disabled: false,
                    home_path: "data/users/1000".into(),
                    unlocked: true,
                }],
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);
    });

    let mut client = DebugClient::new(UnixSocketTransport::from_stream(client_stream));
    let users = client.list_users().unwrap();
    assert_eq!(users[0].uid, 1000);
    assert!(users[0].unlocked);
    server.join().unwrap();
}

#[test]
fn client_records_trace_with_chunked_stop() {
    let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut buffer = Vec::new();
        let (frame, used) = read_frame(&mut server_stream, &mut buffer);
        buffer.drain(..used);
        assert_eq!(frame.method_id, METHOD_TRACE_START);
        let start = decode_trace_start_request(&frame.payload).unwrap();
        assert_eq!(start.buffer_mode, 2);
        assert_eq!(start.output_format, 1);
        let mut payload = Vec::new();
        encode_debug_status(
            &DebugStatusResponse {
                status: 0,
                message: "ok".into(),
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);

        let (frame, used) = read_frame(&mut server_stream, &mut buffer);
        buffer.drain(..used);
        assert_eq!(frame.method_id, METHOD_TRACE_STATUS);
        let mut payload = Vec::new();
        encode_trace_status_response(
            &TraceStatusResponse {
                status: 0,
                state: 2,
                categories: start.categories,
                buffer_mode: start.buffer_mode,
                buffer_size_kb: start.buffer_size_kb,
                output_format: start.output_format,
                producer_count: 1,
                event_count: 2,
                dropped_count: 0,
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);

        let trace = b"FXT\0BEXOS\0trace-bytes".to_vec();
        let (frame, used) = read_frame(&mut server_stream, &mut buffer);
        buffer.drain(..used);
        assert_eq!(frame.method_id, METHOD_TRACE_STOP);
        let stop = decode_trace_stop_request(&frame.payload).unwrap();
        assert_eq!(stop.offset, 0);
        let mut payload = Vec::new();
        encode_trace_stop_response(
            &TraceStopResponse {
                status: 0,
                offset: 0,
                total_len: trace.len() as u64,
                bytes: trace[..10].to_vec(),
                complete: false,
                output_format: start.output_format,
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);

        let (frame, _) = read_frame(&mut server_stream, &mut buffer);
        assert_eq!(frame.method_id, METHOD_TRACE_STOP);
        let stop = decode_trace_stop_request(&frame.payload).unwrap();
        assert_eq!(stop.offset, 10);
        let mut payload = Vec::new();
        encode_trace_stop_response(
            &TraceStopResponse {
                status: 0,
                offset: 10,
                total_len: trace.len() as u64,
                bytes: trace[10..].to_vec(),
                complete: true,
                output_format: start.output_format,
            },
            &mut payload,
        );
        write_response(&mut server_stream, &frame, payload);
    });

    let mut client = DebugClient::new(UnixSocketTransport::from_stream(client_stream));
    client.trace_start(0x42, 2, 2048).unwrap();
    assert_eq!(client.trace_status().unwrap().event_count, 2);
    let trace = client.trace_stop().unwrap();
    assert!(trace.starts_with(b"FXT\0BEXOS\0"));
    server.join().unwrap();
}

fn read_frame(server_stream: &mut UnixStream, buffer: &mut Vec<u8>) -> (Frame, usize) {
    let mut temp = [0u8; 256];
    loop {
        if let Ok(parsed) = parse_frame(buffer) {
            return parsed;
        }
        let n = server_stream.read(&mut temp).unwrap();
        buffer.extend_from_slice(&temp[..n]);
    }
}

fn write_response(server_stream: &mut UnixStream, frame: &Frame, payload: Vec<u8>) {
    let mut response = Vec::new();
    Frame {
        flags: 0,
        request_id: frame.request_id,
        method_id: frame.method_id,
        payload,
    }
    .encode(&mut response)
    .unwrap();
    server_stream.write_all(&response).unwrap();
}

#[test]
fn shell_client_rejects_overconsumption_and_survives_fragmented_response() {
    use bexos_debug_wire::*;
    let (a, mut b) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        let mut buf = Vec::new();
        let frame = loop {
            let mut chunk = [0; 256];
            let n = b.read(&mut chunk).unwrap();
            buf.extend_from_slice(&chunk[..n]);
            if let Ok((f, _)) = parse_frame(&buf) {
                break f;
            }
        };
        let q = decode_shell_request(&frame.payload).unwrap();
        assert_eq!(q.input, b"abc");
        let mut payload = Vec::new();
        encode_shell_response(
            &ShellResponse {
                session_id: 1,
                consumed: 4,
                ..Default::default()
            },
            &mut payload,
        );
        let mut bytes = Vec::new();
        Frame { payload, ..frame }.encode(&mut bytes).unwrap();
        for chunk in bytes.chunks(3) {
            b.write_all(chunk).unwrap();
        }
    });
    let mut c = DebugClient::new(UnixSocketTransport::from_stream(a));
    assert!(c.exchange_shell(1, b"abc", false).is_err());
    server.join().unwrap();
}
