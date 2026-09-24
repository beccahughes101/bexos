//! Decode each frozen inline layout before adapting it to the current envelope.
use bootstrap_fidl::{FidlDecode, HandleRef, Startup, WireVector};
use kernel_fidl::Status;
pub fn decode<'a>(bytes: &'a [u8], handles: &'a [HandleRef]) -> Result<Startup<'a>, Status> {
    let version = u32::from_le_bytes(
        bytes
            .get(..4)
            .ok_or(Status::ErrInvalidArgs)?
            .try_into()
            .unwrap(),
    );
    macro_rules! base {
        ($s:expr) => {{
            let s = $s;
            Startup {
                version: s.version,
                resources: s.resources,
                arg0: s.arg0,
                arg1: s.arg1,
                namespace: s.namespace,
                namespace_paths: s.namespace_paths,
                migration: s.migration,
                migration_generation: s.migration_generation,
                migration_target: s.migration_target,
                service_grant_endpoints: s.service_grant_endpoints,
                service_grant_descriptors: s.service_grant_descriptors,
                config: s.config,
                config_len: s.config_len,
                config_endpoint: s.config_endpoint,
                linker_data: s.linker_data,
                linker_data_len: s.linker_data_len,
                trace_producer: &[],
                trace_mapped_len: 0,
                trace_producer_id: 0,
                trace_pid: 0,
                trace_main_tid: 0,
                driver_resources: WireVector::from_slice(&[]),
                driver_lifecycle: &[],
                incoming_service_endpoints: &[],
                incoming_service_descriptors: "",
                lazy_idle_timeout_ms: 0,
                lazy_generation: 0,
                locale_data: &[],
                locale_data_len: 0,
                locale_data_generation: 0,
                locale_settings: &[],
                driver_host_controller: &[],
                driver_recovery: &[],
            }
        }};
    }
    macro_rules! trace {
        ($o:ident,$s:ident) => {
            $o.trace_producer = $s.trace_producer;
            $o.trace_mapped_len = $s.trace_mapped_len;
            $o.trace_producer_id = $s.trace_producer_id;
            $o.trace_pid = $s.trace_pid;
            $o.trace_main_tid = $s.trace_main_tid;
        };
    }
    macro_rules! driver {
        ($o:ident,$s:ident) => {
            $o.driver_resources = $s.driver_resources;
            $o.driver_lifecycle = $s.driver_lifecycle;
        };
    }
    macro_rules! read {
        ($ty:ident) => {
            bootstrap_fidl::$ty::decode(bytes, handles).map_err(|_| Status::ErrInvalidArgs)?
        };
    }
    Ok(match version {
        6 => base!(read!(StartupV6)),
        7 => {
            let s = read!(StartupV7);
            let mut out = base!(s);
            trace!(out, s);
            out
        }
        8 => {
            let s = read!(StartupV8);
            let mut out = base!(s);
            trace!(out, s);
            driver!(out, s);
            out
        }
        9 => {
            let s = read!(StartupV9);
            let mut out = base!(s);
            trace!(out, s);
            driver!(out, s);
            out.incoming_service_endpoints = s.incoming_service_endpoints;
            out.incoming_service_descriptors = s.incoming_service_descriptors;
            out.lazy_idle_timeout_ms = s.lazy_idle_timeout_ms;
            out.lazy_generation = s.lazy_generation;
            out
        }
        10 => {
            let s = read!(StartupV10);
            let mut out = base!(s);
            trace!(out, s);
            driver!(out, s);
            out.incoming_service_endpoints = s.incoming_service_endpoints;
            out.incoming_service_descriptors = s.incoming_service_descriptors;
            out.lazy_idle_timeout_ms = s.lazy_idle_timeout_ms;
            out.lazy_generation = s.lazy_generation;
            out.locale_data = s.locale_data;
            out.locale_data_len = s.locale_data_len;
            out.locale_data_generation = s.locale_data_generation;
            out.locale_settings = s.locale_settings;
            out
        }
        11 => read!(Startup),
        _ => return Err(Status::ErrInvalidArgs),
    })
}
