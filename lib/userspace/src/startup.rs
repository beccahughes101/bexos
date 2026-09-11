use crate::{Channel, Memory, dynamic_link};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use bootstrap_fidl::HardwareResourceKind;
use bootstrap_fidl::{FidlDecode, FidlEncode, HandleRef};
use kernel_fidl::Status;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupHardwareResource {
    pub kind: HardwareResourceKind,
    pub resource_id: u64,
    pub base: u64,
    pub length: u64,
    pub flags: u64,
    pub handle: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamespaceEntry {
    pub path: String,
    pub directory: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceGrant {
    pub service: String,
    pub protocol: String,
    pub capability: String,
    pub method_ordinals: Vec<u64>,
    pub permission_values: Vec<String>,
    pub caller_package: Option<String>,
    pub caller_uid: Option<u64>,
    pub caller_foreground: bool,
    pub endpoint: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceProducerDescriptor {
    pub buffer: u64,
    pub mapped_len: u64,
    pub producer_id: u64,
    pub pid: u64,
    pub main_tid: u64,
}

pub struct Startup {
    pub resources: Vec<u64>,
    pub driver_resources: Vec<StartupHardwareResource>,
    pub driver_lifecycle: Option<Channel>,
    pub arg0: u64,
    pub arg1: u64,
    pub namespace: Vec<NamespaceEntry>,
    pub namespace_paths: String,
    pub config: Option<u64>,
    pub config_len: u64,
    pub config_endpoint: Option<Channel>,
    pub linker_data: Option<u64>,
    pub linker_data_len: u64,
    pub migration: Option<Channel>,
    pub migration_generation: u64,
    pub migration_target: bool,
    pub service_grants: Vec<ServiceGrant>,
    pub incoming_service_grants: Vec<ServiceGrant>,
    pub lazy_idle_timeout_ms: u32,
    pub lazy_generation: u64,
    pub trace_producer: Option<TraceProducerDescriptor>,
}
impl Startup {
    pub fn receive(channel: Channel) -> Result<Self, Status> {
        let m = channel.recv_blocking()?;
        let hs: Vec<_> = m.handles.iter().map(|h| HandleRef { raw: *h }).collect();
        match get_u32(&m.bytes, 0)? {
            2 => decode_v2_startup(&m.bytes, &hs),
            3 => decode_v3_startup(&m.bytes, &hs),
            4 | 5 => decode_v4_or_v5_startup(&m.bytes, &hs),
            6 | 7 | 8 | 9 => decode_v6_or_later_startup(&m.bytes, &hs),
            _ => Err(Status::ErrInvalidArgs),
        }
    }
    pub fn send(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        paths: &str,
    ) -> Result<(), Status> {
        Self::send_migratable(channel, resources, arg0, arg1, paths, None, 0, false)
    }
    pub fn send_with_config(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        paths: &str,
        config: Option<(u64, u64)>,
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_config_and_linker_data(
            channel,
            resources,
            arg0,
            arg1,
            paths,
            None,
            0,
            false,
            &[],
            config,
            None,
        )
    }

    pub fn send_with_namespace(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        namespace: &[NamespaceEntry],
        config: Option<(u64, u64)>,
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_namespace_config_and_linker_data(
            channel,
            resources,
            arg0,
            arg1,
            namespace,
            None,
            0,
            false,
            &[],
            config,
            None,
            None,
        )
    }
    pub fn send_migratable(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        paths: &str,
        migration: Option<Channel>,
        generation: u64,
        target: bool,
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_config_and_linker_data(
            channel,
            resources,
            arg0,
            arg1,
            paths,
            migration,
            generation,
            target,
            &[],
            None,
            None,
        )
    }

    pub fn send_with_service_grants(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        paths: &str,
        service_grants: &[ServiceGrant],
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_config_and_linker_data(
            channel,
            resources,
            arg0,
            arg1,
            paths,
            None,
            0,
            false,
            service_grants,
            None,
            None,
        )
    }

    pub fn send_migratable_with_service_grants(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        paths: &str,
        migration: Option<Channel>,
        generation: u64,
        target: bool,
        service_grants: &[ServiceGrant],
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_config_and_linker_data(
            channel,
            resources,
            arg0,
            arg1,
            paths,
            migration,
            generation,
            target,
            service_grants,
            None,
            None,
        )
    }

    pub fn send_migratable_with_service_grants_and_config(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        paths: &str,
        migration: Option<Channel>,
        generation: u64,
        target: bool,
        service_grants: &[ServiceGrant],
        config: Option<(u64, u64)>,
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_config_and_linker_data(
            channel,
            resources,
            arg0,
            arg1,
            paths,
            migration,
            generation,
            target,
            service_grants,
            config,
            None,
        )
    }

    pub fn send_migratable_with_service_grants_config_and_linker_data(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        paths: &str,
        migration: Option<Channel>,
        generation: u64,
        target: bool,
        service_grants: &[ServiceGrant],
        config: Option<(u64, u64)>,
        linker_data: Option<(u64, u64)>,
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_namespace_config_and_linker_data(
            channel,
            resources,
            arg0,
            arg1,
            &namespace_from_path_and_handles(paths, resources)?,
            migration,
            generation,
            target,
            service_grants,
            config,
            None,
            linker_data,
        )
    }

    pub fn send_migratable_with_service_grants_config_linker_and_trace_data(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        paths: &str,
        migration: Option<Channel>,
        generation: u64,
        target: bool,
        service_grants: &[ServiceGrant],
        config: Option<(u64, u64)>,
        linker_data: Option<(u64, u64)>,
        trace_producer: Option<TraceProducerDescriptor>,
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_namespace_config_linker_and_trace_data(
            channel,
            resources,
            arg0,
            arg1,
            &namespace_from_path_and_handles(paths, resources)?,
            migration,
            generation,
            target,
            service_grants,
            config,
            None,
            linker_data,
            trace_producer,
        )
    }

    pub fn send_migratable_with_service_grants_namespace_config_and_linker_data(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        namespace: &[NamespaceEntry],
        migration: Option<Channel>,
        generation: u64,
        target: bool,
        service_grants: &[ServiceGrant],
        config: Option<(u64, u64)>,
        config_endpoint: Option<Channel>,
        linker_data: Option<(u64, u64)>,
    ) -> Result<(), Status> {
        Self::send_migratable_with_service_grants_namespace_config_linker_and_trace_data(
            channel,
            resources,
            arg0,
            arg1,
            namespace,
            migration,
            generation,
            target,
            service_grants,
            config,
            config_endpoint,
            linker_data,
            None,
        )
    }

    pub fn send_migratable_with_service_grants_namespace_config_linker_and_trace_data(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        namespace: &[NamespaceEntry],
        migration: Option<Channel>,
        generation: u64,
        target: bool,
        service_grants: &[ServiceGrant],
        config: Option<(u64, u64)>,
        config_endpoint: Option<Channel>,
        linker_data: Option<(u64, u64)>,
        trace_producer: Option<TraceProducerDescriptor>,
    ) -> Result<(), Status> {
        Self::send_full(
            channel,
            resources,
            &[],
            None,
            arg0,
            arg1,
            namespace,
            migration,
            generation,
            target,
            service_grants,
            config,
            config_endpoint,
            linker_data,
            trace_producer,
            &[],
            0,
            0,
        )
    }

    pub fn send_lazy_provider_startup(
        channel: Channel,
        resources: &[u64],
        arg0: u64,
        arg1: u64,
        namespace: &[NamespaceEntry],
        migration: Option<Channel>,
        generation: u64,
        service_grants: &[ServiceGrant],
        incoming_service_grants: &[ServiceGrant],
        lazy_idle_timeout_ms: u32,
        lazy_generation: u64,
        config: Option<(u64, u64)>,
        config_endpoint: Option<Channel>,
        linker_data: Option<(u64, u64)>,
        trace_producer: Option<TraceProducerDescriptor>,
    ) -> Result<(), Status> {
        Self::send_full(
            channel,
            resources,
            &[],
            None,
            arg0,
            arg1,
            namespace,
            migration,
            generation,
            false,
            service_grants,
            config,
            config_endpoint,
            linker_data,
            trace_producer,
            incoming_service_grants,
            lazy_idle_timeout_ms,
            lazy_generation,
        )
    }

    pub fn send_d1_driver(
        channel: Channel,
        driver_resources: &[StartupHardwareResource],
        lifecycle: Channel,
        node_id: u64,
        migration: Option<Channel>,
        generation: u64,
        target: bool,
        service_grants: &[ServiceGrant],
        linker_data: Option<(u64, u64)>,
        trace_producer: Option<TraceProducerDescriptor>,
    ) -> Result<(), Status> {
        Self::send_full(
            channel,
            &[],
            driver_resources,
            Some(lifecycle),
            0,
            node_id,
            &[],
            migration,
            generation,
            target,
            service_grants,
            None,
            None,
            linker_data,
            trace_producer,
            &[],
            0,
            0,
        )
    }

    fn send_full(
        channel: Channel,
        resources: &[u64],
        driver_resources: &[StartupHardwareResource],
        driver_lifecycle: Option<Channel>,
        arg0: u64,
        arg1: u64,
        namespace: &[NamespaceEntry],
        migration: Option<Channel>,
        generation: u64,
        target: bool,
        service_grants: &[ServiceGrant],
        config: Option<(u64, u64)>,
        config_endpoint: Option<Channel>,
        linker_data: Option<(u64, u64)>,
        trace_producer: Option<TraceProducerDescriptor>,
        incoming_service_grants: &[ServiceGrant],
        lazy_idle_timeout_ms: u32,
        lazy_generation: u64,
    ) -> Result<(), Status> {
        let hs: Vec<_> = resources.iter().map(|h| HandleRef { raw: *h }).collect();
        let driver_resource_handles: Vec<_> = driver_resources
            .iter()
            .map(|resource| HandleRef {
                raw: resource.handle,
            })
            .collect();
        let driver_resource_entries: Vec<_> = driver_resources
            .iter()
            .zip(driver_resource_handles.iter())
            .map(|(resource, handle)| bootstrap_fidl::DriverResource {
                kind: resource.kind,
                resource_id: resource.resource_id,
                base: resource.base,
                length: resource.length,
                flags: resource.flags,
                resource: *handle,
            })
            .collect();
        let lifecycle_handles: Vec<_> = driver_lifecycle
            .into_iter()
            .map(|c| HandleRef { raw: c.0 })
            .collect();
        let namespace_handles: Vec<_> = namespace
            .iter()
            .map(|entry| HandleRef {
                raw: entry.directory,
            })
            .collect();
        let namespace_entries: Vec<_> = namespace
            .iter()
            .zip(namespace_handles.iter())
            .map(|(entry, handle)| bootstrap_fidl::NamespaceEntry {
                path: &entry.path,
                directory: *handle,
            })
            .collect();
        let migration_handles: Vec<_> = migration
            .into_iter()
            .map(|c| HandleRef { raw: c.0 })
            .collect();
        let config_len = config.map(|(_, len)| len).unwrap_or(0);
        let config_handles: Vec<_> = config
            .into_iter()
            .map(|(raw, _)| HandleRef { raw })
            .collect();
        let config_endpoint_handles: Vec<_> = config_endpoint
            .into_iter()
            .map(|c| HandleRef { raw: c.0 })
            .collect();
        let linker_data_len = linker_data.map(|(_, len)| len).unwrap_or(0);
        let linker_data_handles: Vec<_> = linker_data
            .into_iter()
            .map(|(raw, _)| HandleRef { raw })
            .collect();
        let trace_handles: Vec<_> = trace_producer
            .as_ref()
            .into_iter()
            .map(|trace| HandleRef { raw: trace.buffer })
            .collect();
        let service_grant_endpoints: Vec<_> = service_grants
            .iter()
            .map(|grant| HandleRef {
                raw: grant.endpoint,
            })
            .collect();
        let service_grant_descriptors = encode_service_grant_descriptors(service_grants)?;
        let incoming_service_endpoints: Vec<_> = incoming_service_grants
            .iter()
            .map(|grant| HandleRef {
                raw: grant.endpoint,
            })
            .collect();
        let incoming_service_descriptors =
            encode_service_grant_descriptors(incoming_service_grants)?;
        let paths = namespace_paths(namespace)?;
        let s = bootstrap_fidl::Startup {
            version: 9,
            resources: &hs,
            arg0,
            arg1,
            namespace: bootstrap_fidl::WireVector::from_slice(&namespace_entries),
            namespace_paths: &paths,
            migration: &migration_handles,
            migration_generation: generation,
            migration_target: target,
            service_grant_endpoints: &service_grant_endpoints,
            service_grant_descriptors: &service_grant_descriptors,
            config: &config_handles,
            config_len,
            config_endpoint: &config_endpoint_handles,
            linker_data: &linker_data_handles,
            linker_data_len,
            trace_producer: &trace_handles,
            trace_mapped_len: trace_producer
                .as_ref()
                .map(|trace| trace.mapped_len)
                .unwrap_or(0),
            trace_producer_id: trace_producer
                .as_ref()
                .map(|trace| trace.producer_id)
                .unwrap_or(0),
            trace_pid: trace_producer.as_ref().map(|trace| trace.pid).unwrap_or(0),
            trace_main_tid: trace_producer
                .as_ref()
                .map(|trace| trace.main_tid)
                .unwrap_or(0),
            driver_resources: bootstrap_fidl::WireVector::from_slice(&driver_resource_entries),
            driver_lifecycle: &lifecycle_handles,
            incoming_service_endpoints: &incoming_service_endpoints,
            incoming_service_descriptors: &incoming_service_descriptors,
            lazy_idle_timeout_ms,
            lazy_generation,
        };
        let mut bytes = [0; 4096];
        let mut handles = [HandleRef { raw: 0 }; 80];
        let e = s
            .encode(&mut bytes, &mut handles)
            .map_err(|_| Status::ErrInvalidArgs)?;
        channel.send(
            &bytes[..e.bytes],
            &handles[..e.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<Vec<_>>(),
        )
    }
    pub fn ready(channel: Channel) -> Result<(), Status> {
        channel.send(&0i32.to_le_bytes(), &[])
    }
    pub fn wait_ready(channel: Channel) -> Result<(), Status> {
        let m = channel.recv_blocking()?;
        if m.bytes != 0i32.to_le_bytes() {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(())
    }
    pub fn close_resources(self) {
        let mut closed = Vec::new();
        for h in self.resources {
            close_once(h, &mut closed);
        }
        for resource in self.driver_resources {
            close_once(resource.handle, &mut closed);
        }
        for entry in self.namespace {
            close_once(entry.directory, &mut closed);
        }
        if let Some(channel) = self.config_endpoint {
            close_once(channel.0, &mut closed);
        }
        if let Some(channel) = self.driver_lifecycle {
            close_once(channel.0, &mut closed);
        }
    }
}

fn close_once(handle: u64, closed: &mut Vec<u64>) {
    if handle != 0 && !closed.contains(&handle) {
        closed.push(handle);
        let _ = Memory::close(handle);
    }
}

fn decode_namespace_entries(
    entries: bootstrap_fidl::WireVector<'_, bootstrap_fidl::NamespaceEntry<'_>>,
) -> Result<Vec<NamespaceEntry>, Status> {
    let mut out = Vec::new();
    for index in 0..entries.len() {
        let entry = entries.get(index).map_err(|_| Status::ErrInvalidArgs)?;
        if entry.path.is_empty() || entry.directory.raw == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        if out
            .iter()
            .any(|prior: &NamespaceEntry| prior.path == entry.path)
        {
            return Err(Status::ErrInvalidArgs);
        }
        out.push(NamespaceEntry {
            path: entry.path.to_string(),
            directory: entry.directory.raw,
        });
    }
    Ok(out)
}

fn namespace_from_legacy_paths(
    paths: &str,
    resources: &[u64],
) -> Result<Vec<NamespaceEntry>, Status> {
    namespace_from_path_and_handles(paths, resources)
}

fn namespace_from_path_and_handles(
    paths: &str,
    handles: &[u64],
) -> Result<Vec<NamespaceEntry>, Status> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (index, path) in paths.split(';').enumerate() {
        let Some(directory) = handles.get(index).copied() else {
            return Err(Status::ErrInvalidArgs);
        };
        if path.is_empty() || directory == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        out.push(NamespaceEntry {
            path: path.to_string(),
            directory,
        });
    }
    Ok(out)
}

fn namespace_paths(entries: &[NamespaceEntry]) -> Result<String, Status> {
    let mut out = String::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.path.is_empty() || entry.path.contains(';') || entry.directory == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        if index != 0 {
            out.push(';');
        }
        out.push_str(&entry.path);
    }
    if out.len() > 256 {
        Err(Status::ErrInvalidArgs)
    } else {
        Ok(out)
    }
}

fn encode_service_grant_descriptors(grants: &[ServiceGrant]) -> Result<String, Status> {
    let mut out = String::new();
    for (index, grant) in grants.iter().enumerate() {
        if grant.service.contains('|')
            || grant.protocol.contains('|')
            || grant.capability.contains('|')
            || grant.service.contains(';')
            || grant.protocol.contains(';')
            || grant.capability.contains(';')
            || grant
                .caller_package
                .as_deref()
                .is_some_and(|value| value.contains('|') || value.contains(';'))
            || grant.permission_values.iter().any(|value| {
                value.contains('|')
                    || value.contains(';')
                    || value.contains(',')
                    || value.contains('=')
            })
        {
            return Err(Status::ErrInvalidArgs);
        }
        if index != 0 {
            out.push(';');
        }
        out.push_str(&grant.service);
        out.push('|');
        out.push_str(&grant.protocol);
        out.push('|');
        out.push_str(&grant.capability);
        out.push('|');
        for (ordinal_index, ordinal) in grant.method_ordinals.iter().enumerate() {
            if ordinal_index != 0 {
                out.push(',');
            }
            out.push_str(&ordinal.to_string());
        }
        out.push('|');
        for (value_index, value) in grant.permission_values.iter().enumerate() {
            if value_index != 0 {
                out.push(',');
            }
            out.push_str(value);
        }
        if grant.caller_package.is_some() || grant.caller_uid.is_some() || !grant.caller_foreground
        {
            out.push('|');
            if let Some(package) = &grant.caller_package {
                out.push_str(package);
            }
            out.push('|');
            if let Some(uid) = grant.caller_uid {
                out.push_str(&uid.to_string());
            }
            out.push('|');
            out.push_str(if grant.caller_foreground { "fg" } else { "bg" });
        }
    }
    if out.len() > 1024 {
        Err(Status::ErrInvalidArgs)
    } else {
        Ok(out)
    }
}

fn parse_service_grant_descriptors(value: &str) -> Result<Vec<ServiceGrant>, Status> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut grants = Vec::new();
    for entry in value.split(';') {
        let mut parts = entry.split('|');
        let service = parts.next().ok_or(Status::ErrInvalidArgs)?;
        let protocol = parts.next().ok_or(Status::ErrInvalidArgs)?;
        let capability = parts.next().ok_or(Status::ErrInvalidArgs)?;
        let ordinals = parts.next().ok_or(Status::ErrInvalidArgs)?;
        let values = parts.next().unwrap_or("");
        let caller_package = parts.next().and_then(|value| {
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        });
        let caller_uid = match parts
            .next()
            .and_then(|value| if value.is_empty() { None } else { Some(value) })
        {
            Some(value) => Some(value.parse::<u64>().map_err(|_| Status::ErrInvalidArgs)?),
            None => None,
        };
        let caller_foreground = parts
            .next()
            .map(|value| value == "fg" || value == "1")
            .unwrap_or(true);
        if parts.next().is_some()
            || service.is_empty()
            || protocol.is_empty()
            || capability.is_empty()
        {
            return Err(Status::ErrInvalidArgs);
        }
        let mut method_ordinals = Vec::new();
        if !ordinals.is_empty() {
            for ordinal in ordinals.split(',') {
                method_ordinals.push(ordinal.parse::<u64>().map_err(|_| Status::ErrInvalidArgs)?);
            }
        }
        grants.push(ServiceGrant {
            service: service.to_string(),
            protocol: protocol.to_string(),
            capability: capability.to_string(),
            method_ordinals,
            permission_values: if values.is_empty() {
                Vec::new()
            } else {
                values.split(',').map(str::to_string).collect()
            },
            caller_package,
            caller_uid,
            caller_foreground,
            endpoint: 0,
        });
    }
    Ok(grants)
}

fn decode_v2_startup(bytes: &[u8], handles: &[HandleRef]) -> Result<Startup, Status> {
    if get_u32(bytes, 0)? != 2 {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(Startup {
        resources: decode_handle_vector(bytes, handles, 4)?,
        driver_resources: Vec::new(),
        driver_lifecycle: None,
        arg0: get_u64(bytes, 20)?,
        arg1: get_u64(bytes, 28)?,
        namespace: namespace_from_legacy_paths(
            decode_string(bytes, 36)?,
            &decode_handle_vector(bytes, handles, 4)?,
        )?,
        namespace_paths: decode_string(bytes, 36)?.to_string(),
        config: None,
        config_len: 0,
        config_endpoint: None,
        linker_data: None,
        linker_data_len: 0,
        migration: decode_handle_vector(bytes, handles, 52)?
            .first()
            .copied()
            .map(Channel),
        migration_generation: get_u64(bytes, 68)?,
        migration_target: *bytes.get(76).ok_or(Status::ErrInvalidArgs)? != 0,
        service_grants: Vec::new(),
        incoming_service_grants: Vec::new(),
        lazy_idle_timeout_ms: 0,
        lazy_generation: 0,
        trace_producer: None,
    })
}

fn decode_v3_startup(bytes: &[u8], handles: &[HandleRef]) -> Result<Startup, Status> {
    Ok(Startup {
        resources: decode_handle_vector(bytes, handles, 4)?,
        driver_resources: Vec::new(),
        driver_lifecycle: None,
        arg0: get_u64(bytes, 20)?,
        arg1: get_u64(bytes, 28)?,
        namespace: namespace_from_legacy_paths(
            decode_string(bytes, 36)?,
            &decode_handle_vector(bytes, handles, 4)?,
        )?,
        namespace_paths: decode_string(bytes, 36)?.to_string(),
        config: None,
        config_len: 0,
        config_endpoint: None,
        linker_data: None,
        linker_data_len: 0,
        migration: decode_handle_vector(bytes, handles, 52)?
            .first()
            .copied()
            .map(Channel),
        migration_generation: get_u64(bytes, 68)?,
        migration_target: *bytes.get(76).ok_or(Status::ErrInvalidArgs)? != 0,
        service_grants: {
            let endpoints = decode_handle_vector(bytes, handles, 77)?;
            let descriptors = parse_service_grant_descriptors(decode_string(bytes, 93)?)?;
            if descriptors.len() != endpoints.len() {
                return Err(Status::ErrInvalidArgs);
            }
            descriptors
                .into_iter()
                .zip(endpoints)
                .map(|(mut grant, endpoint)| {
                    grant.endpoint = endpoint;
                    grant
                })
                .collect()
        },
        incoming_service_grants: Vec::new(),
        lazy_idle_timeout_ms: 0,
        lazy_generation: 0,
        trace_producer: None,
    })
}

fn decode_v4_or_v5_startup(bytes: &[u8], handles: &[HandleRef]) -> Result<Startup, Status> {
    let version = get_u32(bytes, 0)?;
    if version != 4 && version != 5 {
        return Err(Status::ErrInvalidArgs);
    }
    let resources = decode_handle_vector(bytes, handles, 4)?;
    let endpoints = decode_handle_vector(bytes, handles, 77)?;
    let descriptors = parse_service_grant_descriptors(decode_string(bytes, 93)?)?;
    if descriptors.len() != endpoints.len() {
        return Err(Status::ErrInvalidArgs);
    }
    let config = decode_handle_vector(bytes, handles, 109)?;
    let linker_data = if version >= 5 {
        decode_handle_vector(bytes, handles, 133)?
    } else {
        Vec::new()
    };
    let startup = Startup {
        resources: resources.clone(),
        driver_resources: Vec::new(),
        driver_lifecycle: None,
        arg0: get_u64(bytes, 20)?,
        arg1: get_u64(bytes, 28)?,
        namespace: namespace_from_legacy_paths(decode_string(bytes, 36)?, &resources)?,
        namespace_paths: decode_string(bytes, 36)?.to_string(),
        config: config.first().copied(),
        config_len: get_u64(bytes, 125)?,
        config_endpoint: None,
        linker_data: linker_data.first().copied(),
        linker_data_len: if version >= 5 {
            get_u64(bytes, 149)?
        } else {
            0
        },
        migration: decode_handle_vector(bytes, handles, 52)?
            .first()
            .copied()
            .map(Channel),
        migration_generation: get_u64(bytes, 68)?,
        migration_target: *bytes.get(76).ok_or(Status::ErrInvalidArgs)? != 0,
        service_grants: descriptors
            .into_iter()
            .zip(endpoints)
            .map(|(mut grant, endpoint)| {
                grant.endpoint = endpoint;
                grant
            })
            .collect(),
        incoming_service_grants: Vec::new(),
        lazy_idle_timeout_ms: 0,
        lazy_generation: 0,
        trace_producer: None,
    };
    if let Some(handle) = startup.linker_data {
        dynamic_link::install_from_vmo(handle, startup.linker_data_len)?;
    }
    Ok(startup)
}

fn decode_v6_or_later_startup(bytes: &[u8], handles: &[HandleRef]) -> Result<Startup, Status> {
    let s = bootstrap_fidl::Startup::decode(bytes, handles).map_err(|_| Status::ErrInvalidArgs)?;
    if s.version != 6 && s.version != 7 && s.version != 8 && s.version != 9 {
        return Err(Status::ErrInvalidArgs);
    }
    let descriptors = parse_service_grant_descriptors(s.service_grant_descriptors)?;
    if descriptors.len() != s.service_grant_endpoints.len() {
        return Err(Status::ErrInvalidArgs);
    }
    let mut service_grants = Vec::new();
    for (index, mut grant) in descriptors.into_iter().enumerate() {
        grant.endpoint = s.service_grant_endpoints[index].raw;
        service_grants.push(grant);
    }
    let mut incoming_service_grants = Vec::new();
    if s.version >= 9 {
        let descriptors = parse_service_grant_descriptors(s.incoming_service_descriptors)?;
        if descriptors.len() != s.incoming_service_endpoints.len() {
            return Err(Status::ErrInvalidArgs);
        }
        for (index, mut grant) in descriptors.into_iter().enumerate() {
            grant.endpoint = s.incoming_service_endpoints[index].raw;
            incoming_service_grants.push(grant);
        }
    }
    let startup = Startup {
        resources: s.resources.iter().map(|h| h.raw).collect(),
        driver_resources: if s.version >= 8 {
            decode_driver_resources(s.driver_resources)?
        } else {
            Vec::new()
        },
        driver_lifecycle: if s.version >= 8 {
            s.driver_lifecycle.first().map(|h| Channel(h.raw))
        } else {
            None
        },
        arg0: s.arg0,
        arg1: s.arg1,
        namespace: decode_namespace_entries(s.namespace)?,
        namespace_paths: s.namespace_paths.to_string(),
        config: s.config.first().map(|h| h.raw),
        config_len: s.config_len,
        config_endpoint: s.config_endpoint.first().map(|h| Channel(h.raw)),
        linker_data: s.linker_data.first().map(|h| h.raw),
        linker_data_len: s.linker_data_len,
        migration: s.migration.first().map(|h| Channel(h.raw)),
        migration_generation: s.migration_generation,
        migration_target: s.migration_target,
        service_grants,
        incoming_service_grants,
        lazy_idle_timeout_ms: if s.version >= 9 {
            s.lazy_idle_timeout_ms
        } else {
            0
        },
        lazy_generation: if s.version >= 9 { s.lazy_generation } else { 0 },
        trace_producer: (s.version >= 7 && !s.trace_producer.is_empty()).then(|| {
            TraceProducerDescriptor {
                buffer: s.trace_producer[0].raw,
                mapped_len: s.trace_mapped_len,
                producer_id: s.trace_producer_id,
                pid: s.trace_pid,
                main_tid: s.trace_main_tid,
            }
        }),
    };
    if let Some(handle) = startup.linker_data {
        dynamic_link::install_from_vmo(handle, startup.linker_data_len)?;
    }
    if let Some(trace) = &startup.trace_producer {
        install_trace_writer(trace)?;
    }
    Ok(startup)
}

fn decode_driver_resources(
    resources: bootstrap_fidl::WireVector<'_, bootstrap_fidl::DriverResource>,
) -> Result<Vec<StartupHardwareResource>, Status> {
    let mut out = Vec::new();
    for index in 0..resources.len() {
        let resource = resources.get(index).map_err(|_| Status::ErrInvalidArgs)?;
        if resource.resource.raw == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        if out
            .iter()
            .any(|prior: &StartupHardwareResource| prior.resource_id == resource.resource_id)
        {
            return Err(Status::ErrInvalidArgs);
        }
        out.push(StartupHardwareResource {
            kind: resource.kind,
            resource_id: resource.resource_id,
            base: resource.base,
            length: resource.length,
            flags: resource.flags,
            handle: resource.resource.raw,
        });
    }
    Ok(out)
}

fn install_trace_writer(trace: &TraceProducerDescriptor) -> Result<(), Status> {
    if trace.buffer == 0 || trace.mapped_len == 0 {
        return Err(Status::ErrInvalidArgs);
    }
    let mapped = Memory::map(trace.buffer, trace.mapped_len, 2 | 4)?;
    unsafe {
        bexos_trace::init_global_writer(
            mapped as *mut u8,
            trace.mapped_len as usize,
            trace.producer_id,
            trace.pid,
            trace.main_tid,
        )
    }
    .ok_or(Status::ErrInvalidArgs)?;
    Ok(())
}

fn decode_handle_vector(
    bytes: &[u8],
    handles: &[HandleRef],
    offset: usize,
) -> Result<Vec<u64>, Status> {
    let start = get_u64(bytes, offset)? as usize;
    let len = get_u64(bytes, offset + 8)? as usize;
    if start.checked_add(len).is_none_or(|end| end > handles.len()) {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(handles[start..start + len].iter().map(|h| h.raw).collect())
}

fn decode_string(bytes: &[u8], offset: usize) -> Result<&str, Status> {
    let start = get_u64(bytes, offset)? as usize;
    let len = get_u64(bytes, offset + 8)? as usize;
    let end = start.checked_add(len).ok_or(Status::ErrInvalidArgs)?;
    core::str::from_utf8(bytes.get(start..end).ok_or(Status::ErrInvalidArgs)?)
        .map_err(|_| Status::ErrInvalidArgs)
}

fn get_u32(bytes: &[u8], offset: usize) -> Result<u32, Status> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or(Status::ErrInvalidArgs)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn get_u64(bytes: &[u8], offset: usize) -> Result<u64, Status> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or(Status::ErrInvalidArgs)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}
