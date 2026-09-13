use crate::startup_compat::decode;
use bootstrap_fidl::{FidlEncode, HandleRef, WireVector};
#[test]
fn startup_v6_wire_layout() {
    let source = bootstrap_fidl::StartupV6 {
        version: 6,
        resources: &[HandleRef { raw: 41 }],
        arg0: 7,
        arg1: 8,
        namespace: WireVector::from_slice(&[]),
        namespace_paths: "",
        migration: &[],
        migration_generation: 0,
        migration_target: false,
        service_grant_endpoints: &[],
        service_grant_descriptors: "",
        config: &[],
        config_len: 0,
        config_endpoint: &[],
        linker_data: &[],
        linker_data_len: 0,
    };
    let mut bytes = [0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let n = source.encode(&mut bytes, &mut handles).unwrap();
    let result = decode(&bytes[..n.bytes], &handles[..n.handles]).unwrap();
    assert_eq!(result.version, 6);
    assert_eq!(result.resources[0].raw, 41);
    assert_eq!(result.arg0, 7);
    assert_eq!(result.arg1, 8);
    assert!(result.locale_data.is_empty());
    assert!(result.locale_settings.is_empty());
}
#[test]
fn startup_v7_wire_layout() {
    let source = bootstrap_fidl::StartupV7 {
        version: 7,
        resources: &[HandleRef { raw: 41 }],
        arg0: 7,
        arg1: 8,
        namespace: WireVector::from_slice(&[]),
        namespace_paths: "",
        migration: &[],
        migration_generation: 0,
        migration_target: false,
        service_grant_endpoints: &[],
        service_grant_descriptors: "",
        config: &[],
        config_len: 0,
        config_endpoint: &[],
        linker_data: &[],
        linker_data_len: 0,
        trace_producer: &[],
        trace_mapped_len: 0,
        trace_producer_id: 0,
        trace_pid: 0,
        trace_main_tid: 0,
    };
    let mut bytes = [0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let n = source.encode(&mut bytes, &mut handles).unwrap();
    let result = decode(&bytes[..n.bytes], &handles[..n.handles]).unwrap();
    assert_eq!(result.version, 7);
    assert_eq!(result.resources[0].raw, 41);
    assert_eq!(result.arg0, 7);
    assert_eq!(result.arg1, 8);
    assert!(result.locale_data.is_empty());
    assert!(result.locale_settings.is_empty());
}
#[test]
fn startup_v8_wire_layout() {
    let source = bootstrap_fidl::StartupV8 {
        version: 8,
        resources: &[HandleRef { raw: 41 }],
        arg0: 7,
        arg1: 8,
        namespace: WireVector::from_slice(&[]),
        namespace_paths: "",
        migration: &[],
        migration_generation: 0,
        migration_target: false,
        service_grant_endpoints: &[],
        service_grant_descriptors: "",
        config: &[],
        config_len: 0,
        config_endpoint: &[],
        linker_data: &[],
        linker_data_len: 0,
        trace_producer: &[],
        trace_mapped_len: 0,
        trace_producer_id: 0,
        trace_pid: 0,
        trace_main_tid: 0,
        driver_resources: WireVector::from_slice(&[]),
        driver_lifecycle: &[],
    };
    let mut bytes = [0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let n = source.encode(&mut bytes, &mut handles).unwrap();
    let result = decode(&bytes[..n.bytes], &handles[..n.handles]).unwrap();
    assert_eq!(result.version, 8);
    assert_eq!(result.resources[0].raw, 41);
    assert_eq!(result.arg0, 7);
    assert_eq!(result.arg1, 8);
    assert!(result.locale_data.is_empty());
    assert!(result.locale_settings.is_empty());
}
#[test]
fn startup_v9_wire_layout() {
    let source = bootstrap_fidl::StartupV9 {
        version: 9,
        resources: &[HandleRef { raw: 41 }],
        arg0: 7,
        arg1: 8,
        namespace: WireVector::from_slice(&[]),
        namespace_paths: "",
        migration: &[],
        migration_generation: 0,
        migration_target: false,
        service_grant_endpoints: &[],
        service_grant_descriptors: "",
        config: &[],
        config_len: 0,
        config_endpoint: &[],
        linker_data: &[],
        linker_data_len: 0,
        trace_producer: &[],
        trace_mapped_len: 0,
        trace_producer_id: 0,
        trace_pid: 0,
        trace_main_tid: 0,
        driver_resources: WireVector::from_slice(&[]),
        driver_lifecycle: &[],
        incoming_service_endpoints: &[],
        incoming_service_descriptors: "",
        lazy_idle_timeout_ms: 123,
        lazy_generation: 9,
    };
    let mut bytes = [0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let n = source.encode(&mut bytes, &mut handles).unwrap();
    let result = decode(&bytes[..n.bytes], &handles[..n.handles]).unwrap();
    assert_eq!(result.version, 9);
    assert_eq!(result.resources[0].raw, 41);
    assert_eq!(result.arg0, 7);
    assert_eq!(result.arg1, 8);
    assert_eq!(result.lazy_idle_timeout_ms, 123);
    assert_eq!(result.lazy_generation, 9);
    assert!(result.locale_data.is_empty());
    assert!(result.locale_settings.is_empty());
}
#[test]
fn startup_v10_wire_layout() {
    let source = bootstrap_fidl::Startup {
        version: 10,
        resources: &[HandleRef { raw: 41 }],
        arg0: 7,
        arg1: 8,
        namespace: WireVector::from_slice(&[]),
        namespace_paths: "",
        migration: &[],
        migration_generation: 0,
        migration_target: false,
        service_grant_endpoints: &[],
        service_grant_descriptors: "",
        config: &[],
        config_len: 0,
        config_endpoint: &[],
        linker_data: &[],
        linker_data_len: 0,
        trace_producer: &[],
        trace_mapped_len: 0,
        trace_producer_id: 0,
        trace_pid: 0,
        trace_main_tid: 0,
        driver_resources: WireVector::from_slice(&[]),
        driver_lifecycle: &[],
        incoming_service_endpoints: &[],
        incoming_service_descriptors: "",
        lazy_idle_timeout_ms: 123,
        lazy_generation: 9,
        locale_data: &[HandleRef { raw: 42 }],
        locale_data_len: 4096,
        locale_data_generation: 2,
        locale_settings: b"snapshot",
    };
    let mut bytes = [0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let n = source.encode(&mut bytes, &mut handles).unwrap();
    let result = decode(&bytes[..n.bytes], &handles[..n.handles]).unwrap();
    assert_eq!(result.version, 10);
    assert_eq!(result.resources[0].raw, 41);
    assert_eq!(result.arg0, 7);
    assert_eq!(result.arg1, 8);
    assert_eq!(result.lazy_idle_timeout_ms, 123);
    assert_eq!(result.lazy_generation, 9);
    assert_eq!(result.locale_data[0].raw, 42);
    assert_eq!(result.locale_settings, b"snapshot");
    assert_eq!(result.locale_data_len, 4096);
}
