use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::vec::Vec;

use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
    service_binding::BoundServiceEndpoint,
};
use net_fidl::{IpAddress as WireIpAddress, SocketAddress};

use crate::dns::{
    CacheEntry, CacheKey, CacheValue, PendingQuery, RecordType, ResolverState, ResponseCode,
};
use crate::routing::{
    DnsTransport, DnsUpstream, IpAddress, IpPrefix, Provider, ProviderConfig, Registry, Target,
};

pub(crate) const RUNTIME_KEY: u64 = 0;
pub(crate) const ROUTING_KEY: u64 = 1;
pub(crate) const DNS_KEY: u64 = 2;
pub(crate) const ESCROW_KEY: u64 = 3;
pub(crate) const RECOVERY_KEY: u64 = 4;
const VERSION: u64 = 7;

#[derive(Clone, Debug, PartialEq)]
pub struct EscrowedBackend {
    pub socket: u64,
    pub stack_instance: alloc::string::String,
    pub table: u32,
    pub remote: SocketAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryJournal {
    pub handle: u64,
    pub len: u64,
    pub generation: u64,
}

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub client_domains: BTreeMap<u64, alloc::string::String>,
    pub instance_id: alloc::string::String,
    pub domain_tables: BTreeMap<alloc::string::String, u32>,
    pub routing: Registry,
    pub dns: ResolverState,
    pub backend_channels: BTreeMap<alloc::string::String, u64>,
    pub stack_backend_channels: BTreeMap<alloc::string::String, u64>,
    pub stack_controller_channels: BTreeMap<alloc::string::String, u64>,
    pub vswitch_controller: Option<u64>,
    pub switch_routing_controller: Option<u64>,
    pub switch_extension_controller: Option<u64>,
    pub package_resolver: Option<u64>,
    pub tls_trust: Option<u64>,
    pub escrowed_backends: BTreeMap<u64, EscrowedBackend>,
    pub control_proxies: Vec<ControlProxy>,
    pub proxy_controls: Vec<ProxyControl>,
    pub generation: u64,
    pub next_backend_object: u64,
    pub recovery_journals: BTreeMap<alloc::string::String, RecoveryJournal>,
    pub virtual_ports: Vec<u64>,
    pub last_recovery_checkpoint_ms: u64,
    pub pending_extension: Option<PendingExtensionDeployment>,
    pub next_extension_generation: u64,
    pub queued_extensions: Vec<QueuedExtensionDeployment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionDeploymentKind {
    Firewall,
    Nat,
}

pub struct PendingExtensionDeployment {
    pub caller: u64,
    pub kind: ExtensionDeploymentKind,
    pub config: Vec<u8>,
    pub generation: u64,
    pub stage: u8,
    pub module: Option<u64>,
    pub resolver: Option<u64>,
    pub module_length: u64,
    pub digest: [u8; 32],
}

#[derive(Clone)]
pub struct QueuedExtensionDeployment {
    pub kind: ExtensionDeploymentKind,
    pub registry_host: alloc::string::String,
    pub repository: alloc::string::String,
    pub tag: alloc::string::String,
    pub expected_digest: [u8; 32],
    pub config: Vec<u8>,
}

pub struct ControlProxy {
    pub front: Channel,
    pub back: Channel,
    pub flow_id: u64,
    pub escrow_control: u64,
    pub backend: u64,
    pub stack_instance: alloc::string::String,
    pub table: u32,
    pub kind: u8,
}

pub struct ProxyControl {
    pub channel: Channel,
    pub stream: Option<u64>,
    pub flow_id: u64,
    pub backend: Option<u64>,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>) -> Self {
        Self {
            control,
            migration,
            clients: Vec::new(),
            client_domains: BTreeMap::new(),
            instance_id: "system_default".into(),
            domain_tables: BTreeMap::new(),
            routing: Registry::new(64),
            dns: ResolverState::new(256),
            backend_channels: BTreeMap::new(),
            stack_backend_channels: BTreeMap::new(),
            stack_controller_channels: BTreeMap::new(),
            vswitch_controller: None,
            switch_routing_controller: None,
            switch_extension_controller: None,
            package_resolver: None,
            tls_trust: None,
            escrowed_backends: BTreeMap::new(),
            control_proxies: Vec::new(),
            proxy_controls: Vec::new(),
            generation: 0,
            next_backend_object: 1 << 63,
            recovery_journals: BTreeMap::new(),
            virtual_ports: Vec::new(),
            last_recovery_checkpoint_ms: 0,
            pending_extension: None,
            next_extension_generation: 2,
            queued_extensions: Vec::new(),
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self::new(Channel(0), None)
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![RUNTIME_KEY, ROUTING_KEY, DNS_KEY, ESCROW_KEY, RECOVERY_KEY]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        match key {
            RUNTIME_KEY => {
                w.word(VERSION);
                w.word(if cfg!(bexos_arch_x86_64) { 2 } else { 1 });
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |channel| channel.0));
                w.word(self.generation);
                w.word(self.next_backend_object);
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.channel.0);
                    w.text(&client.protocol);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
                w.text(&self.instance_id);
                w.word(self.domain_tables.len() as u64);
                for (domain, table) in &self.domain_tables {
                    w.text(domain);
                    w.word(u64::from(*table));
                }
                w.word(self.client_domains.len() as u64);
                for (channel, domain) in &self.client_domains {
                    w.word(*channel);
                    w.text(domain);
                }
            }
            ROUTING_KEY => encode_routing(&mut w, &self.routing),
            DNS_KEY => encode_dns(&mut w, &self.dns),
            ESCROW_KEY => {
                w.word(VERSION);
                w.word(self.backend_channels.len() as u64);
                for (instance, channel) in &self.backend_channels {
                    w.text(instance);
                    w.word(*channel);
                }
                w.word(self.vswitch_controller.unwrap_or(0));
                w.word(self.tls_trust.unwrap_or(0));
                w.word(self.switch_routing_controller.unwrap_or(0));
                w.word(self.switch_extension_controller.unwrap_or(0));
                w.word(self.package_resolver.unwrap_or(0));
                w.word(self.stack_backend_channels.len() as u64);
                for (instance, channel) in &self.stack_backend_channels {
                    w.text(instance);
                    w.word(*channel);
                }
                w.word(self.stack_controller_channels.len() as u64);
                for (instance, channel) in &self.stack_controller_channels {
                    w.text(instance);
                    w.word(*channel);
                }
                w.word(self.escrowed_backends.len() as u64);
                for (flow, backend) in &self.escrowed_backends {
                    w.word(*flow);
                    w.word(backend.socket);
                    w.text(&backend.stack_instance);
                    w.word(u64::from(backend.table));
                    encode_wire_socket_address(&mut w, backend.remote);
                }
                w.word(self.control_proxies.len() as u64);
                for proxy in &self.control_proxies {
                    w.word(proxy.front.0);
                    w.word(proxy.back.0);
                    w.word(proxy.flow_id);
                    w.word(proxy.escrow_control);
                    w.word(proxy.backend);
                    w.text(&proxy.stack_instance);
                    w.word(u64::from(proxy.table));
                    w.word(u64::from(proxy.kind));
                }
                w.word(self.proxy_controls.len() as u64);
                for proxy in &self.proxy_controls {
                    w.word(proxy.channel.0);
                    w.word(proxy.stream.unwrap_or(0));
                    w.word(proxy.flow_id);
                    w.word(proxy.backend.unwrap_or(0));
                }
                w.word(self.next_extension_generation);
                w.word(self.pending_extension.is_some() as u64);
                if let Some(pending) = &self.pending_extension {
                    w.word(pending.caller);
                    w.word(match pending.kind {
                        ExtensionDeploymentKind::Firewall => 1,
                        ExtensionDeploymentKind::Nat => 2,
                    });
                    w.bytes(&pending.config);
                    w.word(pending.generation);
                    w.word(pending.stage as u64);
                    w.word(pending.module.unwrap_or(0));
                    w.word(pending.resolver.unwrap_or(0));
                    w.word(pending.module_length);
                    w.bytes(&pending.digest);
                }
                w.word(self.queued_extensions.len() as u64);
                for queued in &self.queued_extensions {
                    w.word(match queued.kind {
                        ExtensionDeploymentKind::Firewall => 1,
                        ExtensionDeploymentKind::Nat => 2,
                    });
                    w.text(&queued.registry_host);
                    w.text(&queued.repository);
                    w.text(&queued.tag);
                    w.bytes(&queued.expected_digest);
                    w.bytes(&queued.config);
                }
            }
            RECOVERY_KEY => {
                w.word(VERSION);
                w.word(self.recovery_journals.len() as u64);
                for (instance, journal) in &self.recovery_journals {
                    w.text(instance);
                    w.word(journal.handle);
                    w.word(journal.len);
                    w.word(journal.generation);
                }
                w.word(self.virtual_ports.len() as u64);
                for port in &self.virtual_ports {
                    w.word(*port);
                }
                w.word(self.last_recovery_checkpoint_ms);
            }
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        match key {
            RUNTIME_KEY => {
                let version = read_version(&mut r)?;
                let architecture = r.word()?;
                if architecture != if cfg!(bexos_arch_x86_64) { 2 } else { 1 } {
                    return Err(Error::InvalidData);
                }
                self.control = Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                self.generation = r.word()?;
                self.next_backend_object = if version >= 3 { r.word()? } else { 1 << 63 };
                self.clients.clear();
                for _ in 0..r.count(256)? {
                    let channel = Channel(r.word()?);
                    let protocol = r.text(64)?.to_string();
                    let mut ordinals = Vec::new();
                    for _ in 0..r.count(64)? {
                        ordinals.push(r.word()?);
                    }
                    self.clients.push(BoundServiceEndpoint::new_with_protocol(
                        channel, ordinals, &protocol,
                    ));
                }
                if version >= 2 {
                    self.instance_id = r.text(64)?.to_string();
                    self.domain_tables.clear();
                    for _ in 0..r.count(64)? {
                        self.domain_tables.insert(
                            r.text(64)?.to_string(),
                            u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                        );
                    }
                }
                self.client_domains.clear();
                for _ in 0..r.count(256)? {
                    self.client_domains
                        .insert(r.word()?, r.text(64)?.to_string());
                }
            }
            ROUTING_KEY => self.routing = decode_routing(&mut r)?,
            DNS_KEY => self.dns = decode_dns(&mut r)?,
            ESCROW_KEY => {
                let version = read_version(&mut r)?;
                self.backend_channels.clear();
                for _ in 0..r.count(64)? {
                    self.backend_channels
                        .insert(r.text(64)?.to_string(), r.word()?);
                }
                self.vswitch_controller = if version >= 2 {
                    let channel = r.word()?;
                    (channel != 0).then_some(channel)
                } else {
                    None
                };
                self.tls_trust = if version >= 3 {
                    let channel = r.word()?;
                    (channel != 0).then_some(channel)
                } else {
                    None
                };
                if version >= 7 {
                    let routing = r.word()?;
                    self.switch_routing_controller = (routing != 0).then_some(routing);
                    let extensions = r.word()?;
                    self.switch_extension_controller = (extensions != 0).then_some(extensions);
                    let resolver = r.word()?;
                    self.package_resolver = (resolver != 0).then_some(resolver);
                }
                self.stack_backend_channels.clear();
                self.stack_controller_channels.clear();
                if version >= 2 {
                    for _ in 0..r.count(64)? {
                        self.stack_backend_channels
                            .insert(r.text(64)?.to_string(), r.word()?);
                    }
                    for _ in 0..r.count(64)? {
                        self.stack_controller_channels
                            .insert(r.text(64)?.to_string(), r.word()?);
                    }
                }
                self.escrowed_backends.clear();
                for _ in 0..r.count(4096)? {
                    let flow = r.word()?;
                    let socket = r.word()?;
                    let backend = if version >= 4 {
                        EscrowedBackend {
                            socket,
                            stack_instance: r.text(64)?.to_string(),
                            table: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                            remote: decode_wire_socket_address(&mut r)?,
                        }
                    } else {
                        EscrowedBackend {
                            socket,
                            stack_instance: self.instance_id.clone(),
                            table: 0,
                            remote: SocketAddress {
                                addr: WireIpAddress::Ipv4(net_fidl::Ipv4Address { octets: [0; 4] }),
                                port: 0,
                            },
                        }
                    };
                    self.escrowed_backends.insert(flow, backend);
                }
                self.control_proxies.clear();
                for _ in 0..r.count(4096)? {
                    let front = Channel(r.word()?);
                    let back = Channel(r.word()?);
                    let flow_id = r.word()?;
                    let (escrow_control, backend, stack_instance, table, kind) = if version >= 6 {
                        (
                            r.word()?,
                            r.word()?,
                            r.text(64)?.to_string(),
                            u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                            u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                        )
                    } else {
                        (0, 0, self.instance_id.clone(), 0, 0)
                    };
                    self.control_proxies.push(ControlProxy {
                        front,
                        back,
                        flow_id,
                        escrow_control,
                        backend,
                        stack_instance,
                        table,
                        kind,
                    });
                }
                self.proxy_controls.clear();
                for _ in 0..r.count(4096)? {
                    let channel = Channel(r.word()?);
                    let stream = r.word()?;
                    self.proxy_controls.push(ProxyControl {
                        channel,
                        stream: (stream != 0).then_some(stream),
                        flow_id: r.word()?,
                        backend: {
                            let backend = r.word()?;
                            (backend != 0).then_some(backend)
                        },
                    });
                }
                if version >= 7 {
                    self.next_extension_generation = r.word()?;
                    self.pending_extension = if r.flag()? {
                        Some(PendingExtensionDeployment {
                            caller: r.word()?,
                            kind: match r.word()? {
                                1 => ExtensionDeploymentKind::Firewall,
                                2 => ExtensionDeploymentKind::Nat,
                                _ => return Err(Error::InvalidData),
                            },
                            config: r.bytes(65_536)?.to_vec(),
                            generation: r.word()?,
                            stage: u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                            module: {
                                let handle = r.word()?;
                                (handle != 0).then_some(handle)
                            },
                            resolver: {
                                let handle = r.word()?;
                                (handle != 0).then_some(handle)
                            },
                            module_length: r.word()?,
                            digest: r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?,
                        })
                    } else {
                        None
                    };
                    self.queued_extensions.clear();
                    for _ in 0..r.count(2)? {
                        self.queued_extensions.push(QueuedExtensionDeployment {
                            kind: match r.word()? {
                                1 => ExtensionDeploymentKind::Firewall,
                                2 => ExtensionDeploymentKind::Nat,
                                _ => return Err(Error::InvalidData),
                            },
                            registry_host: r.text(128)?.to_string(),
                            repository: r.text(128)?.to_string(),
                            tag: r.text(64)?.to_string(),
                            expected_digest: r
                                .bytes(32)?
                                .try_into()
                                .map_err(|_| Error::InvalidData)?,
                            config: r.bytes(65_536)?.to_vec(),
                        });
                    }
                }
            }
            RECOVERY_KEY => {
                let version = read_version(&mut r)?;
                if version < 5 {
                    return Err(Error::UnsupportedVersion);
                }
                self.recovery_journals.clear();
                for _ in 0..r.count(64)? {
                    self.recovery_journals.insert(
                        r.text(64)?.to_string(),
                        RecoveryJournal {
                            handle: r.word()?,
                            len: r.word()?,
                            generation: r.word()?,
                        },
                    );
                }
                self.virtual_ports.clear();
                for _ in 0..r.count(64)? {
                    self.virtual_ports.push(r.word()?);
                }
                self.last_recovery_checkpoint_ms = r.word()?;
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()?;
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = Vec::new();
        out.push(Resource::Handle(self.control.0));
        if let Some(migration) = self.migration {
            out.push(Resource::Handle(migration.0));
        }
        for client in &self.clients {
            out.push(Resource::Handle(client.channel.0));
        }
        for channel in self.backend_channels.values().copied() {
            out.push(Resource::Handle(channel));
        }
        for channel in self.stack_backend_channels.values().copied() {
            out.push(Resource::Handle(channel));
        }
        for channel in self.stack_controller_channels.values().copied() {
            out.push(Resource::Handle(channel));
        }
        if let Some(channel) = self.vswitch_controller {
            out.push(Resource::Handle(channel));
        }
        for channel in [
            self.switch_routing_controller,
            self.switch_extension_controller,
            self.package_resolver,
        ]
        .into_iter()
        .flatten()
        {
            out.push(Resource::Handle(channel));
        }
        if let Some(channel) = self.tls_trust {
            out.push(Resource::Handle(channel));
        }
        for backend in self.escrowed_backends.values() {
            out.push(Resource::Handle(backend.socket));
        }
        for proxy in &self.control_proxies {
            out.push(Resource::Handle(proxy.front.0));
            out.push(Resource::Handle(proxy.back.0));
            if proxy.escrow_control != 0 {
                out.push(Resource::Handle(proxy.escrow_control));
            }
        }
        for proxy in &self.proxy_controls {
            out.push(Resource::Handle(proxy.channel.0));
            if let Some(stream) = proxy.stream {
                out.push(Resource::Handle(stream));
            }
        }
        if let Some(handle) = self
            .pending_extension
            .as_ref()
            .and_then(|pending| pending.module)
        {
            out.push(Resource::Handle(handle));
        }
        if let Some(handle) = self
            .pending_extension
            .as_ref()
            .and_then(|pending| pending.resolver)
        {
            out.push(Resource::Handle(handle));
        }
        for provider in self.routing.providers.values() {
            if let Target::Proxy { channel } = provider.config.target {
                out.push(Resource::Handle(channel));
            }
        }
        for journal in self.recovery_journals.values() {
            out.push(Resource::Handle(journal.handle));
        }
        out
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self
                .routing
                .flow_provider
                .values()
                .any(|token| !self.routing.providers.contains_key(token))
        {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn activated(&mut self, generation: u64) {
        self.generation = generation;
    }
}

fn read_version(r: &mut Decoder<'_>) -> Result<u64, Error> {
    match r.word()? {
        1 => Ok(1),
        2 => Ok(2),
        3 => Ok(3),
        4 => Ok(4),
        5 => Ok(5),
        6 => Ok(6),
        VERSION => Ok(VERSION),
        _ => Err(Error::UnsupportedVersion),
    }
}

fn expect_version(r: &mut Decoder<'_>) -> Result<(), Error> {
    read_version(r).map(|_| ())
}

fn encode_routing(w: &mut Encoder, registry: &Registry) {
    w.word(VERSION);
    w.word(registry.max_providers as u64);
    w.word(registry.next_token);
    w.word(registry.next_flow_id);
    w.word(registry.providers.len() as u64);
    for provider in registry.providers.values() {
        w.word(provider.token);
        w.text(&provider.name);
        w.word(provider.generation);
        w.word(provider.accepting as u64);
        w.word(provider.active_flows);
        encode_provider_config(w, &provider.config);
    }
    w.word(registry.flow_provider.len() as u64);
    for (flow, provider) in &registry.flow_provider {
        w.word(*flow);
        w.word(*provider);
    }
}

fn decode_routing(r: &mut Decoder<'_>) -> Result<Registry, Error> {
    expect_version(r)?;
    let max_providers = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let mut registry = Registry::new(max_providers);
    registry.next_token = r.word()?;
    registry.next_flow_id = r.word()?;
    for _ in 0..r.count(max_providers.min(256))? {
        let token = r.word()?;
        let name = r.text(64)?.to_string();
        let generation = r.word()?;
        let accepting = r.flag()?;
        let active_flows = r.word()?;
        let config = decode_provider_config(r)?;
        registry.providers.insert(
            token,
            Provider {
                token,
                name,
                config,
                generation,
                accepting,
                active_flows,
            },
        );
    }
    for _ in 0..r.count(4096)? {
        let flow = r.word()?;
        let provider = r.word()?;
        if !registry.providers.contains_key(&provider) {
            return Err(Error::InvalidData);
        }
        registry.flow_provider.insert(flow, provider);
    }
    Ok(registry)
}

fn encode_provider_config(w: &mut Encoder, config: &ProviderConfig) {
    match &config.target {
        Target::Device {
            stack_instance,
            table,
            interface_id,
        } => {
            w.word(1);
            w.text(stack_instance);
            w.word(u64::from(*table));
            w.word(*interface_id);
        }
        Target::Proxy { channel } => {
            w.word(2);
            w.word(*channel);
        }
    }
    w.word(u64::from(config.priority));
    w.word(config.default_route as u64);
    w.word(config.ip_routes.len() as u64);
    for prefix in &config.ip_routes {
        encode_ip(w, prefix.address);
        w.word(u64::from(prefix.prefix_len));
    }
    w.word(config.domain_routes.len() as u64);
    for suffix in &config.domain_routes {
        w.text(suffix);
    }
    w.word(config.dns_upstreams.len() as u64);
    for upstream in &config.dns_upstreams {
        encode_ip(w, upstream.address);
        w.word(u64::from(upstream.port));
        w.word(match upstream.transport {
            DnsTransport::Udp53 => 1,
            DnsTransport::Dot => 2,
            DnsTransport::Doh => 3,
        });
        w.text(&upstream.tls_server_name);
        w.text(&upstream.doh_path);
    }
}

fn decode_provider_config(r: &mut Decoder<'_>) -> Result<ProviderConfig, Error> {
    let target = match r.word()? {
        1 => Target::Device {
            stack_instance: r.text(64)?.to_string(),
            table: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            interface_id: r.word()?,
        },
        2 => Target::Proxy { channel: r.word()? },
        _ => return Err(Error::InvalidData),
    };
    let priority = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let default_route = r.flag()?;
    let mut ip_routes = Vec::new();
    for _ in 0..r.count(32)? {
        let address = decode_ip(r)?;
        let prefix_len = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        ip_routes.push(IpPrefix::new(address, prefix_len).map_err(|_| Error::InvalidData)?);
    }
    let mut domain_routes = Vec::new();
    for _ in 0..r.count(32)? {
        domain_routes.push(r.text(253)?.to_string());
    }
    let mut dns_upstreams = Vec::new();
    for _ in 0..r.count(8)? {
        let address = decode_ip(r)?;
        let port = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let transport = match r.word()? {
            1 => DnsTransport::Udp53,
            2 => DnsTransport::Dot,
            3 => DnsTransport::Doh,
            _ => return Err(Error::InvalidData),
        };
        dns_upstreams.push(DnsUpstream {
            address,
            port,
            transport,
            tls_server_name: r.text(128)?.to_string(),
            doh_path: r.text(255)?.to_string(),
        });
    }
    Ok(ProviderConfig {
        target,
        ip_routes,
        domain_routes,
        dns_upstreams,
        priority,
        default_route,
    })
}

fn encode_dns(w: &mut Encoder, state: &ResolverState) {
    w.word(VERSION);
    w.word(state.capacity as u64);
    w.word(state.cache.len() as u64);
    for (key, entry) in &state.cache {
        encode_cache_key(w, key);
        w.word(entry.expires_at_ms);
        w.word(entry.provider_generation);
        match &entry.value {
            CacheValue::Positive(addresses) => {
                w.word(1);
                w.word(addresses.len() as u64);
                for address in addresses {
                    encode_ip(w, *address);
                }
            }
            CacheValue::Negative(code) => {
                w.word(2);
                w.word(encode_rcode(*code));
            }
        }
    }
    w.word(state.pending.len() as u64);
    for pending in state.pending.values() {
        encode_cache_key(w, &pending.key);
        w.word(u64::from(pending.dns_id));
        w.word(pending.deadline_ms);
        w.word(pending.provider_generation);
        w.word(pending.upstream_index as u64);
        w.bytes(&pending.encoded_request);
    }
}

fn decode_dns(r: &mut Decoder<'_>) -> Result<ResolverState, Error> {
    expect_version(r)?;
    let capacity = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let mut state = ResolverState::new(capacity);
    for _ in 0..r.count(capacity.min(4096))? {
        let key = decode_cache_key(r)?;
        let expires_at_ms = r.word()?;
        let provider_generation = r.word()?;
        let value = match r.word()? {
            1 => {
                let mut addresses = Vec::new();
                for _ in 0..r.count(8)? {
                    addresses.push(decode_ip(r)?);
                }
                CacheValue::Positive(addresses)
            }
            2 => CacheValue::Negative(decode_rcode(r.word()?)?),
            _ => return Err(Error::InvalidData),
        };
        state.cache.insert(
            key,
            CacheEntry {
                value,
                expires_at_ms,
                provider_generation,
            },
        );
    }
    for _ in 0..r.count(capacity.min(4096))? {
        let key = decode_cache_key(r)?;
        let dns_id = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let deadline_ms = r.word()?;
        let provider_generation = r.word()?;
        let upstream_index = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let encoded_request = r.bytes(crate::dns::MAX_DNS_MESSAGE)?.to_vec();
        state.pending.insert(
            key.clone(),
            PendingQuery {
                key,
                dns_id,
                deadline_ms,
                provider_generation,
                upstream_index,
                encoded_request,
            },
        );
    }
    Ok(state)
}

fn encode_cache_key(w: &mut Encoder, key: &CacheKey) {
    w.word(u64::from(key.table));
    w.word(key.provider);
    w.text(&key.hostname);
    w.word(key.record_type as u64);
}

fn decode_cache_key(r: &mut Decoder<'_>) -> Result<CacheKey, Error> {
    Ok(CacheKey {
        table: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        provider: r.word()?,
        hostname: r.text(253)?.to_string(),
        record_type: match r.word()? {
            1 => RecordType::A,
            28 => RecordType::Aaaa,
            _ => return Err(Error::InvalidData),
        },
    })
}

fn encode_ip(w: &mut Encoder, address: IpAddress) {
    match address {
        IpAddress::V4(bytes) => {
            w.word(4);
            w.bytes(&bytes);
        }
        IpAddress::V6(bytes) => {
            w.word(6);
            w.bytes(&bytes);
        }
    }
}

fn decode_ip(r: &mut Decoder<'_>) -> Result<IpAddress, Error> {
    match r.word()? {
        4 => Ok(IpAddress::V4(
            r.bytes(4)?.try_into().map_err(|_| Error::InvalidData)?,
        )),
        6 => Ok(IpAddress::V6(
            r.bytes(16)?.try_into().map_err(|_| Error::InvalidData)?,
        )),
        _ => Err(Error::InvalidData),
    }
}

fn encode_wire_socket_address(w: &mut Encoder, address: SocketAddress) {
    match address.addr {
        WireIpAddress::Ipv4(value) => {
            w.word(4);
            w.bytes(&value.octets);
        }
        WireIpAddress::Ipv6(value) => {
            w.word(6);
            w.bytes(&value.octets);
        }
    }
    w.word(u64::from(address.port));
}

fn decode_wire_socket_address(r: &mut Decoder<'_>) -> Result<SocketAddress, Error> {
    let addr = match r.word()? {
        4 => WireIpAddress::Ipv4(net_fidl::Ipv4Address {
            octets: r.bytes(4)?.try_into().map_err(|_| Error::InvalidData)?,
        }),
        6 => WireIpAddress::Ipv6(net_fidl::Ipv6Address {
            octets: r.bytes(16)?.try_into().map_err(|_| Error::InvalidData)?,
        }),
        _ => return Err(Error::InvalidData),
    };
    Ok(SocketAddress {
        addr,
        port: u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
    })
}

fn encode_rcode(code: ResponseCode) -> u64 {
    match code {
        ResponseCode::NoError => 0,
        ResponseCode::ServerFailure => 2,
        ResponseCode::NameError => 3,
        ResponseCode::Refused => 5,
        ResponseCode::Other(code) => 256 + u64::from(code),
    }
}

fn decode_rcode(value: u64) -> Result<ResponseCode, Error> {
    Ok(match value {
        0 => ResponseCode::NoError,
        2 => ResponseCode::ServerFailure,
        3 => ResponseCode::NameError,
        5 => ResponseCode::Refused,
        256..=511 => ResponseCode::Other((value - 256) as u8),
        _ => return Err(Error::InvalidData),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bexos_userspace::live_migration::State;

    #[test]
    fn runtime_and_escrow_round_trip_in_wire_order() {
        let mut source = Runtime::new(Channel(10), Some(Channel(11)));
        source.instance_id = "private".into();
        source.domain_tables.insert("corp.example".into(), 7);
        source.client_domains.insert(12, "corp.example".into());
        source.backend_channels.insert("stack-a".into(), 20);
        source.stack_backend_channels.insert("stack-a".into(), 21);
        source
            .stack_controller_channels
            .insert("stack-a".into(), 22);
        source.vswitch_controller = Some(23);
        source.escrowed_backends.insert(
            99,
            EscrowedBackend {
                socket: 24,
                stack_instance: "stack-a".into(),
                table: 7,
                remote: SocketAddress {
                    addr: WireIpAddress::Ipv4(net_fidl::Ipv4Address {
                        octets: [192, 0, 2, 1],
                    }),
                    port: 443,
                },
            },
        );
        source.control_proxies.push(ControlProxy {
            front: Channel(25),
            back: Channel(26),
            flow_id: 99,
            escrow_control: 30,
            backend: 21,
            stack_instance: "stack-a".into(),
            table: 7,
            kind: 1,
        });
        source.proxy_controls.push(ProxyControl {
            channel: Channel(27),
            stream: Some(28),
            flow_id: 100,
            backend: Some(21),
        });

        let mut target = Runtime::empty();
        source.recovery_journals.insert(
            "stack-a".into(),
            RecoveryJournal {
                handle: 29,
                len: 4096,
                generation: 8,
            },
        );
        source.virtual_ports.push(41);
        for key in [RUNTIME_KEY, ROUTING_KEY, DNS_KEY, ESCROW_KEY, RECOVERY_KEY] {
            let record = source.encode_record(key).unwrap().unwrap();
            target.adopt_record(key, Some(&record)).unwrap();
        }
        assert_eq!(target.instance_id, "private");
        assert_eq!(target.domain_tables.get("corp.example"), Some(&7));
        assert_eq!(
            target.client_domains.get(&12).map(|value| value.as_str()),
            Some("corp.example")
        );
        assert_eq!(target.stack_backend_channels.get("stack-a"), Some(&21));
        assert_eq!(target.stack_controller_channels.get("stack-a"), Some(&22));
        assert_eq!(target.vswitch_controller, Some(23));
        assert_eq!(
            target.escrowed_backends.get(&99),
            source.escrowed_backends.get(&99)
        );
        assert_eq!(
            target.recovery_journals.get("stack-a"),
            source.recovery_journals.get("stack-a")
        );
        assert_eq!(target.virtual_ports, alloc::vec![41]);
        assert_eq!(target.control_proxies.len(), 1);
        assert_eq!(target.proxy_controls.len(), 1);
    }

    #[test]
    fn resources_cover_frontends_backends_and_migration() {
        let mut runtime = Runtime::new(Channel(1), Some(Channel(2)));
        runtime
            .clients
            .push(BoundServiceEndpoint::new(Channel(3), Vec::new()));
        runtime.backend_channels.insert("stack".into(), 4);
        let resources = runtime.resources();
        for expected in [1, 2, 3, 4] {
            assert!(resources.iter().any(|resource| {
                matches!(resource, Resource::Handle(handle) if *handle == expected)
            }));
        }
    }
}
