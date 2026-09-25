use alloc::{string::String, vec::Vec};

use bexos_migration::{Error, codec::Decoder};
use bexos_userspace::{Memory, Startup, config::ConfigTable};

use crate::routing::{DnsTransport, IpAddress};

const MAGIC: u64 = 0x4e45_5450_4f4c_3031;

#[derive(Clone, Debug)]
pub struct BootConfig {
    pub instance_id: String,
    pub max_dynamic_providers: usize,
    pub dns_cache_capacity: usize,
    pub domains: Vec<DomainConfig>,
    pub tables: Vec<u32>,
    pub ports: Vec<PortConfig>,
    pub routes: Vec<RouteConfig>,
    pub upstreams: Vec<UpstreamConfig>,
    pub routed_interfaces: Vec<RoutedInterfaceConfig>,
    pub switch_routes: Vec<RouteConfig>,
    pub firewall_config: Vec<u8>,
    pub firewall_artifact: ExtensionArtifactConfig,
    pub nat_enabled: bool,
    pub nat_config: Vec<u8>,
    pub nat_artifact: ExtensionArtifactConfig,
}

#[derive(Clone, Debug)]
pub struct DomainConfig {
    pub name: String,
    pub table: u32,
    pub system_default: bool,
}

#[derive(Clone, Debug)]
pub struct PortConfig {
    pub id: u64,
    pub table: u32,
    pub physical_interface: u64,
    pub mac: [u8; 6],
    pub vlan_id: u16,
    pub tagged: bool,
    pub rx_queue_depth: u32,
    pub tx_queue_depth: u32,
}

#[derive(Clone, Debug)]
pub struct RouteConfig {
    pub table: u32,
    pub destination: IpAddress,
    pub prefix_len: u8,
    pub gateway: Option<IpAddress>,
    pub interface_id: u64,
    pub metric: u32,
}

#[derive(Clone, Debug)]
pub struct UpstreamConfig {
    pub table: u32,
    pub provider: String,
    pub domain_suffix: String,
    pub address: IpAddress,
    pub port: u16,
    pub transport: DnsTransport,
    pub tls_server_name: String,
    pub doh_path: String,
    pub priority: u32,
}

#[derive(Clone, Debug)]
pub struct RoutedInterfaceConfig {
    pub id: u64,
    pub physical_interface: u64,
    pub virtual_port: u64,
    pub table: u32,
    pub bridge_domain: u32,
    pub vlan_id: u16,
    pub mac: [u8; 6],
    pub mtu: u32,
    pub zone: u16,
    pub addresses: Vec<(IpAddress, u8)>,
}

#[derive(Clone, Debug, Default)]
pub struct ExtensionArtifactConfig {
    pub registry_host: String,
    pub repository: String,
    pub tag: String,
    pub expected_digest: Option<[u8; 32]>,
}

impl BootConfig {
    pub fn from_startup(startup: &Startup) -> Result<Self, Error> {
        let handle = startup.config.ok_or(Error::InvalidData)?;
        if startup.config_len == 0 {
            let _ = Memory::close(handle);
            return Err(Error::InvalidData);
        }
        let mapped_len = startup
            .config_len
            .checked_add(4095)
            .ok_or(Error::InvalidData)?
            & !4095;
        let va = Memory::map(handle, mapped_len, 2).map_err(|_| Error::InvalidData)?;
        let bytes =
            unsafe { core::slice::from_raw_parts(va as *const u8, startup.config_len as usize) };
        let result = (|| {
            let table = ConfigTable::parse(bytes).map_err(|_| Error::InvalidData)?;
            let policy = table
                .get_bytes("boot_policy")
                .map_err(|_| Error::InvalidData)?;
            let mut value = Self::decode(policy)?;
            value.max_dynamic_providers = table
                .get_u32("max_dynamic_providers")
                .unwrap_or(value.max_dynamic_providers as u32)
                as usize;
            value.dns_cache_capacity = table.get_u32("dns_cache_capacity").unwrap_or(256) as usize;
            Ok(value)
        })();
        let _ = Memory::unmap(va, mapped_len);
        let _ = Memory::close(handle);
        result
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != MAGIC {
            return Err(Error::InvalidData);
        }
        let version = r.word()?;
        if !matches!(version, 1 | 2) {
            return Err(Error::UnsupportedVersion);
        }
        let instance_id: String = r.text(64)?.into();
        let max_dynamic_providers = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let mut domains = Vec::new();
        for _ in 0..r.count(64)? {
            domains.push(DomainConfig {
                name: r.text(64)?.into(),
                table: word_u32(&mut r)?,
                system_default: r.word()? != 0,
            });
        }
        let mut tables = Vec::new();
        for _ in 0..r.count(64)? {
            tables.push(word_u32(&mut r)?);
        }
        let mut ports = Vec::new();
        for _ in 0..r.count(64)? {
            let id = r.word()?;
            let table = word_u32(&mut r)?;
            let selector = r.text(128)?;
            let physical_interface =
                bexos_network_policy::selector_id(selector).ok_or(Error::InvalidData)?;
            let mac_bytes = r.bytes(6)?;
            let mac: [u8; 6] = mac_bytes.try_into().map_err(|_| Error::InvalidData)?;
            ports.push(PortConfig {
                id,
                table,
                physical_interface,
                mac,
                vlan_id: u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                tagged: r.word()? != 0,
                rx_queue_depth: word_u32(&mut r)?,
                tx_queue_depth: word_u32(&mut r)?,
            });
        }
        let mut routes = Vec::new();
        for _ in 0..r.count(256)? {
            let table = word_u32(&mut r)?;
            let destination = ip(r.bytes(16)?)?;
            let prefix_len = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let gateway_bytes = r.bytes(16)?;
            let gateway = (!gateway_bytes.is_empty())
                .then(|| ip(gateway_bytes))
                .transpose()?;
            routes.push(RouteConfig {
                table,
                destination,
                prefix_len,
                gateway,
                interface_id: r.word()?,
                metric: word_u32(&mut r)?,
            });
        }
        let mut upstreams = Vec::new();
        for _ in 0..r.count(256)? {
            let table = word_u32(&mut r)?;
            let provider = r.text(64)?.into();
            let domain_suffix = r.text(128)?.into();
            let address = ip(r.bytes(16)?)?;
            let port = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let transport = match r.word()? {
                1 => DnsTransport::Udp53,
                2 => DnsTransport::Dot,
                3 => DnsTransport::Doh,
                _ => return Err(Error::InvalidData),
            };
            upstreams.push(UpstreamConfig {
                table,
                provider,
                domain_suffix,
                address,
                port,
                transport,
                tls_server_name: r.text(128)?.into(),
                doh_path: r.text(255)?.into(),
                priority: word_u32(&mut r)?,
            });
        }
        let mut routed_interfaces = Vec::new();
        let mut switch_routes = Vec::new();
        let mut firewall_config = b"NFW1\0\0\0\0".to_vec();
        let mut firewall_artifact = ExtensionArtifactConfig::default();
        let mut nat_enabled = false;
        let mut nat_config = Vec::new();
        let mut nat_artifact = ExtensionArtifactConfig::default();
        if version >= 2 {
            for _ in 0..r.count(256)? {
                let id = r.word()?;
                let physical_interface = r.word()?;
                let virtual_port = r.word()?;
                let table = word_u32(&mut r)?;
                let bridge_domain = word_u32(&mut r)?;
                let vlan_id = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                let mac = r.bytes(6)?.try_into().map_err(|_| Error::InvalidData)?;
                let mtu = word_u32(&mut r)?;
                let zone = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                let mut addresses = Vec::new();
                for _ in 0..r.count(16)? {
                    addresses.push((
                        ip(r.bytes(16)?)?,
                        u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                    ));
                }
                routed_interfaces.push(RoutedInterfaceConfig {
                    id,
                    physical_interface,
                    virtual_port,
                    table,
                    bridge_domain,
                    vlan_id,
                    mac,
                    mtu,
                    zone,
                    addresses,
                });
            }
            for _ in 0..r.count(4096)? {
                let table = word_u32(&mut r)?;
                let destination = ip(r.bytes(16)?)?;
                let prefix_len = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                let gateway_bytes = r.bytes(16)?;
                let gateway = (!gateway_bytes.is_empty())
                    .then(|| ip(gateway_bytes))
                    .transpose()?;
                switch_routes.push(RouteConfig {
                    table,
                    destination,
                    prefix_len,
                    gateway,
                    interface_id: r.word()?,
                    metric: word_u32(&mut r)?,
                });
            }
            firewall_config = r.bytes(65_536)?.to_vec();
            firewall_artifact = extension_artifact(&mut r)?;
            nat_enabled = r.flag()?;
            nat_config = r.bytes(65_536)?.to_vec();
            nat_artifact = extension_artifact(&mut r)?;
        }
        r.finish()?;
        if instance_id.is_empty()
            || domains.is_empty()
            || tables.is_empty()
            || max_dynamic_providers == 0
            || domains.iter().any(|domain| !tables.contains(&domain.table))
            || ports.iter().any(|port| !tables.contains(&port.table))
            || routes.iter().any(|route| !tables.contains(&route.table))
            || upstreams
                .iter()
                .any(|upstream| !tables.contains(&upstream.table))
            || routed_interfaces
                .iter()
                .any(|interface| !tables.contains(&interface.table))
            || switch_routes
                .iter()
                .any(|route| !tables.contains(&route.table))
            || domains
                .iter()
                .filter(|domain| domain.system_default)
                .count()
                > 1
            || domains
                .iter()
                .any(|domain| domain.system_default && domain.name != "system_default")
        {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            instance_id,
            max_dynamic_providers,
            dns_cache_capacity: 256,
            domains,
            tables,
            ports,
            routes,
            upstreams,
            routed_interfaces,
            switch_routes,
            firewall_config,
            firewall_artifact,
            nat_enabled,
            nat_config,
            nat_artifact,
        })
    }
}

fn extension_artifact(r: &mut Decoder<'_>) -> Result<ExtensionArtifactConfig, Error> {
    let registry_host: String = r.text(128)?.into();
    let repository: String = r.text(128)?.into();
    let tag: String = r.text(64)?.into();
    let digest = r.bytes(32)?;
    let media_type = r.text(128)?;
    let abi = r.text(64)?;
    let absent =
        registry_host.is_empty() && repository.is_empty() && tag.is_empty() && digest.is_empty();
    if !absent
        && (digest.len() != 32
            || media_type != "application/vnd.bexos.net.filter.v1"
            || abi != "vpp-wasm-v1")
    {
        return Err(Error::InvalidData);
    }
    Ok(ExtensionArtifactConfig {
        registry_host,
        repository,
        tag,
        expected_digest: (!digest.is_empty()).then(|| digest.try_into().unwrap()),
    })
}

fn word_u32(r: &mut Decoder<'_>) -> Result<u32, Error> {
    u32::try_from(r.word()?).map_err(|_| Error::InvalidData)
}

fn ip(bytes: &[u8]) -> Result<IpAddress, Error> {
    match bytes.len() {
        4 => Ok(IpAddress::V4(bytes.try_into().unwrap())),
        16 => Ok(IpAddress::V6(bytes.try_into().unwrap())),
        _ => Err(Error::InvalidData),
    }
}
