use alloc::string::String;
use bexos_userspace::{Memory, Startup, config::ConfigTable};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetConfig {
    pub static_ipv4: Option<[u8; 4]>,
    pub static_prefix_len: u8,
    pub static_gateway: Option<[u8; 4]>,
    pub static_dns: Option<[u8; 4]>,
    pub static_ipv6: Option<[u8; 16]>,
    pub static_ipv6_prefix_len: u8,
    pub static_ipv6_gateway: Option<[u8; 16]>,
    pub static_dns_ipv6: Option<[u8; 16]>,
    pub doh_bootstrap_ipv6: Option<[u8; 16]>,
    pub slaac_enabled: bool,
    pub dhcp_enabled: bool,
    pub dhcp_timeout_ms: u32,
    pub mtu: u32,
    pub dns_mode: DnsMode,
    pub doh_host: String,
    pub doh_path: String,
    pub doh_bootstrap_ipv4: Option<[u8; 4]>,
    pub doh_port: u16,
    pub doh_strict: bool,
    pub dns_cache_capacity: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DnsMode {
    Udp53,
    Doh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigSource {
    Static,
    Dhcp,
    Unconfigured,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveConfig {
    pub source: ConfigSource,
    pub ipv4: Option<[u8; 4]>,
    pub prefix_len: u8,
    pub gateway: Option<[u8; 4]>,
    pub dns: Option<[u8; 4]>,
    pub ipv6: Option<[u8; 16]>,
    pub ipv6_prefix_len: u8,
    pub ipv6_gateway: Option<[u8; 16]>,
    pub dns_ipv6: Option<[u8; 16]>,
    pub doh_bootstrap_ipv6: Option<[u8; 16]>,
    pub slaac_enabled: bool,
    pub mtu: u32,
    pub dns_mode: DnsMode,
    pub doh_host: String,
    pub doh_path: String,
    pub doh_bootstrap_ipv4: Option<[u8; 4]>,
    pub doh_port: u16,
    pub doh_strict: bool,
    pub dns_cache_capacity: u32,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            static_ipv4: None,
            static_prefix_len: 24,
            static_gateway: None,
            static_dns: None,
            static_ipv6: None,
            static_ipv6_prefix_len: 64,
            static_ipv6_gateway: None,
            static_dns_ipv6: None,
            doh_bootstrap_ipv6: None,
            slaac_enabled: true,
            dhcp_enabled: true,
            dhcp_timeout_ms: 3000,
            mtu: 1500,
            dns_mode: DnsMode::Udp53,
            doh_host: "dns.google".into(),
            doh_path: "/dns-query".into(),
            doh_bootstrap_ipv4: None,
            doh_port: 443,
            doh_strict: false,
            dns_cache_capacity: 64,
        }
    }
}

impl NetConfig {
    pub fn from_startup(startup: &Startup) -> Self {
        let Some(config) = startup.config else {
            return Self::default();
        };
        if startup.config_len == 0 {
            let _ = Memory::close(config);
            return Self::default();
        }
        let mapped_len = (startup.config_len + 4095) & !4095;
        let Ok(va) = Memory::map(config, mapped_len, 2) else {
            let _ = Memory::close(config);
            return Self::default();
        };
        let bytes =
            unsafe { core::slice::from_raw_parts(va as *const u8, startup.config_len as usize) };
        let parsed = ConfigTable::parse(bytes)
            .map(Self::from_table)
            .unwrap_or_default();
        let _ = Memory::unmap(va, mapped_len);
        let _ = Memory::close(config);
        parsed
    }

    pub fn from_table(table: ConfigTable<'_>) -> Self {
        let mut config = Self::default();
        config.static_ipv4 = table.get_bytes("static_ipv4").ok().and_then(ipv4_bytes);
        config.static_gateway = table.get_bytes("static_gateway").ok().and_then(ipv4_bytes);
        config.static_dns = table.get_bytes("static_dns").ok().and_then(ipv4_bytes);
        config.static_ipv6 = table.get_bytes("static_ipv6").ok().and_then(ipv6_bytes);
        config.static_ipv6_gateway = table
            .get_bytes("static_ipv6_gateway")
            .ok()
            .and_then(ipv6_bytes);
        config.static_dns_ipv6 = table.get_bytes("static_dns_ipv6").ok().and_then(ipv6_bytes);
        config.doh_bootstrap_ipv6 = table
            .get_bytes("doh_bootstrap_ipv6")
            .ok()
            .and_then(ipv6_bytes);
        config.static_prefix_len = table
            .get_u32("static_prefix_len")
            .ok()
            .and_then(|value| u8::try_from(value).ok())
            .filter(|value| *value <= 32)
            .unwrap_or(24);
        config.static_ipv6_prefix_len = table
            .get_u32("static_ipv6_prefix_len")
            .ok()
            .and_then(|value| u8::try_from(value).ok())
            .filter(|value| *value <= 128)
            .unwrap_or(64);
        config.slaac_enabled = table.get_bool("slaac_enabled").unwrap_or(true);
        config.dhcp_enabled = table.get_bool("dhcp_enabled").unwrap_or(true);
        config.dhcp_timeout_ms = table.get_u32("dhcp_timeout_ms").unwrap_or(3000);
        config.mtu = table
            .get_u32("mtu")
            .ok()
            .filter(|value| (576..=9000).contains(value))
            .unwrap_or(1500);
        config.dns_mode = match table.get_string("dns_mode").unwrap_or("udp53") {
            "doh" => DnsMode::Doh,
            _ => DnsMode::Udp53,
        };
        config.doh_host = table
            .get_string("doh_host")
            .ok()
            .filter(|host| !host.is_empty() && host.len() <= 255)
            .unwrap_or("dns.google")
            .into();
        config.doh_path = table
            .get_string("doh_path")
            .ok()
            .filter(|path| path.starts_with('/') && path.len() <= 255)
            .unwrap_or("/dns-query")
            .into();
        config.doh_bootstrap_ipv4 = table
            .get_bytes("doh_bootstrap_ipv4")
            .ok()
            .and_then(ipv4_bytes);
        config.doh_port = table
            .get_u32("doh_port")
            .ok()
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value != 0)
            .unwrap_or(443);
        config.doh_strict = table.get_bool("doh_strict").unwrap_or(false);
        config.dns_cache_capacity = table
            .get_u32("dns_cache_capacity")
            .ok()
            .filter(|value| (1..=1024).contains(value))
            .unwrap_or(64);
        config
    }

    pub fn choose_active(self, dhcp_lease: Option<DhcpLease>) -> ActiveConfig {
        if let Some(ipv4) = self.static_ipv4 {
            return ActiveConfig {
                source: ConfigSource::Static,
                ipv4: Some(ipv4),
                prefix_len: self.static_prefix_len,
                gateway: self.static_gateway,
                dns: self.static_dns,
                ipv6: self
                    .static_ipv6
                    .or_else(|| self.slaac_enabled.then_some(link_local_ipv6())),
                ipv6_prefix_len: if self.static_ipv6.is_some() {
                    self.static_ipv6_prefix_len
                } else {
                    64
                },
                ipv6_gateway: self.static_ipv6_gateway,
                dns_ipv6: self.static_dns_ipv6,
                doh_bootstrap_ipv6: self.doh_bootstrap_ipv6,
                slaac_enabled: self.slaac_enabled,
                mtu: self.mtu,
                dns_mode: self.dns_mode,
                doh_host: self.doh_host,
                doh_path: self.doh_path,
                doh_bootstrap_ipv4: self.doh_bootstrap_ipv4,
                doh_port: self.doh_port,
                doh_strict: self.doh_strict,
                dns_cache_capacity: self.dns_cache_capacity,
            };
        }
        if self.dhcp_enabled {
            if let Some(lease) = dhcp_lease {
                return ActiveConfig {
                    source: ConfigSource::Dhcp,
                    ipv4: Some(lease.ipv4),
                    prefix_len: lease.prefix_len,
                    gateway: lease.gateway,
                    dns: lease.dns,
                    ipv6: self
                        .static_ipv6
                        .or_else(|| self.slaac_enabled.then_some(link_local_ipv6())),
                    ipv6_prefix_len: if self.static_ipv6.is_some() {
                        self.static_ipv6_prefix_len
                    } else {
                        64
                    },
                    ipv6_gateway: self.static_ipv6_gateway,
                    dns_ipv6: self.static_dns_ipv6,
                    doh_bootstrap_ipv6: self.doh_bootstrap_ipv6,
                    slaac_enabled: self.slaac_enabled,
                    mtu: self.mtu,
                    dns_mode: self.dns_mode,
                    doh_host: self.doh_host,
                    doh_path: self.doh_path,
                    doh_bootstrap_ipv4: self.doh_bootstrap_ipv4,
                    doh_port: self.doh_port,
                    doh_strict: self.doh_strict,
                    dns_cache_capacity: self.dns_cache_capacity,
                };
            }
        }
        ActiveConfig {
            source: ConfigSource::Unconfigured,
            ipv4: None,
            prefix_len: 0,
            gateway: None,
            dns: None,
            ipv6: self
                .static_ipv6
                .or_else(|| self.slaac_enabled.then_some(link_local_ipv6())),
            ipv6_prefix_len: if self.static_ipv6.is_some() {
                self.static_ipv6_prefix_len
            } else {
                64
            },
            ipv6_gateway: self.static_ipv6_gateway,
            dns_ipv6: self.static_dns_ipv6,
            doh_bootstrap_ipv6: self.doh_bootstrap_ipv6,
            slaac_enabled: self.slaac_enabled,
            mtu: self.mtu,
            dns_mode: self.dns_mode,
            doh_host: self.doh_host,
            doh_path: self.doh_path,
            doh_bootstrap_ipv4: self.doh_bootstrap_ipv4,
            doh_port: self.doh_port,
            doh_strict: self.doh_strict,
            dns_cache_capacity: self.dns_cache_capacity,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DhcpLease {
    pub ipv4: [u8; 4],
    pub prefix_len: u8,
    pub gateway: Option<[u8; 4]>,
    pub dns: Option<[u8; 4]>,
}

fn ipv4_bytes(bytes: &[u8]) -> Option<[u8; 4]> {
    (bytes.len() == 4).then(|| [bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn ipv6_bytes(bytes: &[u8]) -> Option<[u8; 16]> {
    if bytes.len() != 16 {
        return None;
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(bytes);
    Some(out)
}

fn link_local_ipv6() -> [u8; 16] {
    [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
}
