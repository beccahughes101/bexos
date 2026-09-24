use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub type ProviderToken = u64;
pub type TableId = u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum IpAddress {
    V4([u8; 4]),
    V6([u8; 16]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IpPrefix {
    pub address: IpAddress,
    pub prefix_len: u8,
}

impl IpPrefix {
    pub fn new(address: IpAddress, prefix_len: u8) -> Result<Self, RoutingError> {
        let max = match address {
            IpAddress::V4(_) => 32,
            IpAddress::V6(_) => 128,
        };
        if prefix_len > max {
            return Err(RoutingError::InvalidPrefix);
        }
        Ok(Self {
            address: masked(address, prefix_len),
            prefix_len,
        })
    }

    pub fn contains(&self, address: IpAddress) -> bool {
        masked(address, self.prefix_len) == self.address
    }
}

fn masked(address: IpAddress, prefix_len: u8) -> IpAddress {
    match address {
        IpAddress::V4(mut bytes) => {
            mask_bytes(&mut bytes, prefix_len);
            IpAddress::V4(bytes)
        }
        IpAddress::V6(mut bytes) => {
            mask_bytes(&mut bytes, prefix_len);
            IpAddress::V6(bytes)
        }
    }
}

fn mask_bytes(bytes: &mut [u8], prefix_len: u8) {
    let full = usize::from(prefix_len / 8);
    let partial = prefix_len % 8;
    if partial != 0 && full < bytes.len() {
        bytes[full] &= 0xff << (8 - partial);
    }
    let clear_from = full + usize::from(partial != 0);
    for byte in bytes.iter_mut().skip(clear_from) {
        *byte = 0;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    Device {
        stack_instance: String,
        table: TableId,
        interface_id: u64,
    },
    Proxy {
        channel: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DnsTransport {
    Udp53,
    Dot,
    Doh,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DnsUpstream {
    pub address: IpAddress,
    pub port: u16,
    pub transport: DnsTransport,
    pub tls_server_name: String,
    pub doh_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderConfig {
    pub target: Target,
    pub ip_routes: Vec<IpPrefix>,
    pub domain_routes: Vec<String>,
    pub dns_upstreams: Vec<DnsUpstream>,
    pub priority: u32,
    pub default_route: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Provider {
    pub token: ProviderToken,
    pub name: String,
    pub config: ProviderConfig,
    pub generation: u64,
    pub accepting: bool,
    pub active_flows: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingError {
    InvalidName,
    InvalidDomain,
    InvalidPrefix,
    InvalidTarget,
    InvalidDns,
    Capacity,
    AlreadyExists,
    NotFound,
    NetworkUnreachable,
    ShouldWait,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteLease {
    pub flow_id: u64,
    pub provider_token: ProviderToken,
    pub provider_generation: u64,
    pub target: Target,
    pub normalized_domain: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Destination {
    Ip { address: IpAddress, port: u16 },
    Domain { host: String, port: u16 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Registry {
    pub(crate) providers: BTreeMap<ProviderToken, Provider>,
    pub(crate) flow_provider: BTreeMap<u64, ProviderToken>,
    pub(crate) next_token: ProviderToken,
    pub(crate) next_flow_id: u64,
    pub(crate) max_providers: usize,
}

impl Registry {
    pub fn new(max_providers: usize) -> Self {
        Self {
            providers: BTreeMap::new(),
            flow_provider: BTreeMap::new(),
            next_token: 1,
            next_flow_id: 1,
            max_providers,
        }
    }

    pub fn providers(&self) -> impl Iterator<Item = &Provider> {
        self.providers.values()
    }

    pub fn provider(&self, token: ProviderToken) -> Option<&Provider> {
        self.providers.get(&token)
    }

    pub fn register(
        &mut self,
        name: impl Into<String>,
        mut config: ProviderConfig,
    ) -> Result<ProviderToken, RoutingError> {
        let name = name.into();
        validate_name(&name)?;
        normalize_config(&mut config)?;
        if self.providers.len() >= self.max_providers {
            return Err(RoutingError::Capacity);
        }
        if self
            .providers
            .values()
            .any(|provider| provider.name == name)
        {
            return Err(RoutingError::AlreadyExists);
        }
        let token = self.next_token;
        self.next_token = self
            .next_token
            .checked_add(1)
            .ok_or(RoutingError::Capacity)?;
        self.providers.insert(
            token,
            Provider {
                token,
                name,
                config,
                generation: 1,
                accepting: true,
                active_flows: 0,
            },
        );
        Ok(token)
    }

    /// Validation completes before the live provider is touched, so observers
    /// see the old or new configuration, never a partial update.
    pub fn update(
        &mut self,
        token: ProviderToken,
        mut config: ProviderConfig,
    ) -> Result<u64, RoutingError> {
        normalize_config(&mut config)?;
        let provider = self
            .providers
            .get_mut(&token)
            .ok_or(RoutingError::NotFound)?;
        provider.config = config;
        provider.generation = provider.generation.saturating_add(1);
        provider.accepting = true;
        Ok(provider.generation)
    }

    /// Provider removal is fail-closed for new flows. The provider object is
    /// retained until every pinned flow releases it.
    pub fn remove(&mut self, token: ProviderToken) -> Result<(), RoutingError> {
        let provider = self
            .providers
            .get_mut(&token)
            .ok_or(RoutingError::NotFound)?;
        provider.accepting = false;
        if provider.active_flows == 0 {
            self.providers.remove(&token);
        }
        Ok(())
    }

    pub fn open_flow(&mut self, destination: Destination) -> Result<RouteLease, RoutingError> {
        self.open_flow_in_table(destination, None)
    }

    pub fn open_flow_for_table(
        &mut self,
        destination: Destination,
        table: TableId,
    ) -> Result<RouteLease, RoutingError> {
        self.open_flow_in_table(destination, Some(table))
    }

    fn open_flow_in_table(
        &mut self,
        destination: Destination,
        table: Option<TableId>,
    ) -> Result<RouteLease, RoutingError> {
        let (token, normalized_domain) = match destination {
            Destination::Ip { address, .. } => (self.select_ip(address, table)?, None),
            Destination::Domain { host, .. } => {
                let normalized = normalize_domain(&host)?;
                (self.select_domain(&normalized, table)?, Some(normalized))
            }
        };
        let provider = self
            .providers
            .get_mut(&token)
            .ok_or(RoutingError::NotFound)?;
        if !provider.accepting {
            return Err(RoutingError::NetworkUnreachable);
        }
        let flow_id = self.next_flow_id;
        self.next_flow_id = self
            .next_flow_id
            .checked_add(1)
            .ok_or(RoutingError::Capacity)?;
        provider.active_flows = provider.active_flows.saturating_add(1);
        self.flow_provider.insert(flow_id, token);
        Ok(RouteLease {
            flow_id,
            provider_token: token,
            provider_generation: provider.generation,
            target: provider.config.target.clone(),
            normalized_domain,
        })
    }

    pub fn open_default_flow(&mut self) -> Result<RouteLease, RoutingError> {
        self.open_default_flow_in_table(None)
    }

    pub fn open_default_flow_for_table(
        &mut self,
        table: TableId,
    ) -> Result<RouteLease, RoutingError> {
        self.open_default_flow_in_table(Some(table))
    }

    fn open_default_flow_in_table(
        &mut self,
        table: Option<TableId>,
    ) -> Result<RouteLease, RoutingError> {
        let token = self.select_default(table)?;
        let provider = self
            .providers
            .get_mut(&token)
            .ok_or(RoutingError::NotFound)?;
        let flow_id = self.next_flow_id;
        self.next_flow_id = self
            .next_flow_id
            .checked_add(1)
            .ok_or(RoutingError::Capacity)?;
        provider.active_flows = provider.active_flows.saturating_add(1);
        self.flow_provider.insert(flow_id, token);
        Ok(RouteLease {
            flow_id,
            provider_token: token,
            provider_generation: provider.generation,
            target: provider.config.target.clone(),
            normalized_domain: None,
        })
    }

    pub fn close_flow(&mut self, flow_id: u64) -> Result<(), RoutingError> {
        let token = self
            .flow_provider
            .remove(&flow_id)
            .ok_or(RoutingError::NotFound)?;
        let remove = if let Some(provider) = self.providers.get_mut(&token) {
            provider.active_flows = provider.active_flows.saturating_sub(1);
            provider.active_flows == 0 && !provider.accepting
        } else {
            false
        };
        if remove {
            self.providers.remove(&token);
        }
        Ok(())
    }

    pub fn remove_table(&self, table: TableId) -> Result<(), RoutingError> {
        if self.providers.values().any(|provider| {
            matches!(provider.config.target, Target::Device { table: candidate, .. } if candidate == table)
                && provider.active_flows != 0
        }) {
            Err(RoutingError::ShouldWait)
        } else {
            Ok(())
        }
    }

    pub fn provider_allows(&self, token: ProviderToken, address: IpAddress) -> bool {
        self.providers.get(&token).is_some_and(|provider| {
            provider
                .config
                .ip_routes
                .iter()
                .any(|prefix| prefix.contains(address))
        })
    }

    pub fn select_dns_provider(&self, host: &str) -> Result<ProviderToken, RoutingError> {
        self.select_domain(&normalize_domain(host)?, None)
    }

    fn select_domain(
        &self,
        host: &str,
        table: Option<TableId>,
    ) -> Result<ProviderToken, RoutingError> {
        let mut best: Option<(usize, u32, ProviderToken)> = None;
        let mut matched = false;
        for provider in self
            .providers
            .values()
            .filter(|provider| provider_matches_table(provider, table))
        {
            for suffix in &provider.config.domain_routes {
                if domain_matches(host, suffix) {
                    matched = true;
                    let candidate = (suffix.len(), provider.config.priority, provider.token);
                    if better_domain(candidate, best) {
                        best = Some(candidate);
                    }
                }
            }
        }
        if matched {
            let token = best.expect("matched route has candidate").2;
            return self
                .providers
                .get(&token)
                .filter(|provider| provider.accepting)
                .map(|_| token)
                .ok_or(RoutingError::NetworkUnreachable);
        }
        self.select_default(table)
    }

    fn select_ip(
        &self,
        address: IpAddress,
        table: Option<TableId>,
    ) -> Result<ProviderToken, RoutingError> {
        let mut best: Option<(u8, u32, ProviderToken)> = None;
        for provider in self
            .providers
            .values()
            .filter(|provider| provider.accepting && provider_matches_table(provider, table))
        {
            for prefix in &provider.config.ip_routes {
                if prefix.contains(address) {
                    let candidate = (prefix.prefix_len, provider.config.priority, provider.token);
                    if better_prefix(candidate, best) {
                        best = Some(candidate);
                    }
                }
            }
        }
        match best {
            Some(entry) => Ok(entry.2),
            None => self.select_default(table),
        }
    }

    fn select_default(&self, table: Option<TableId>) -> Result<ProviderToken, RoutingError> {
        self.providers
            .values()
            .filter(|provider| {
                provider.accepting
                    && provider.config.default_route
                    && provider_matches_table(provider, table)
            })
            .min_by_key(|provider| (provider.config.priority, provider.token))
            .map(|provider| provider.token)
            .ok_or(RoutingError::NetworkUnreachable)
    }
}

fn provider_matches_table(provider: &Provider, table: Option<TableId>) -> bool {
    table.is_none_or(|table| {
        matches!(provider.config.target, Target::Proxy { .. })
            || matches!(provider.config.target, Target::Device { table: candidate, .. } if candidate == table)
    })
}

fn better_domain(candidate: (usize, u32, u64), current: Option<(usize, u32, u64)>) -> bool {
    current.is_none_or(|current| {
        candidate.0 > current.0
            || candidate.0 == current.0 && (candidate.1, candidate.2) < (current.1, current.2)
    })
}

fn better_prefix(candidate: (u8, u32, u64), current: Option<(u8, u32, u64)>) -> bool {
    current.is_none_or(|current| {
        candidate.0 > current.0
            || candidate.0 == current.0 && (candidate.1, candidate.2) < (current.1, current.2)
    })
}

fn normalize_config(config: &mut ProviderConfig) -> Result<(), RoutingError> {
    match &config.target {
        Target::Device { stack_instance, .. } if stack_instance.is_empty() => {
            return Err(RoutingError::InvalidTarget);
        }
        Target::Proxy { channel: 0 } => return Err(RoutingError::InvalidTarget),
        _ => {}
    }
    if config.ip_routes.len() > 32
        || config.domain_routes.len() > 32
        || config.dns_upstreams.len() > 8
    {
        return Err(RoutingError::Capacity);
    }
    for prefix in &mut config.ip_routes {
        *prefix = IpPrefix::new(prefix.address, prefix.prefix_len)?;
    }
    for suffix in &mut config.domain_routes {
        *suffix = normalize_domain(suffix)?;
    }
    config.domain_routes.sort();
    config.domain_routes.dedup();
    for upstream in &config.dns_upstreams {
        if upstream.port == 0
            || matches!(upstream.transport, DnsTransport::Dot | DnsTransport::Doh)
                && upstream.tls_server_name.is_empty()
            || upstream.transport == DnsTransport::Doh && !upstream.doh_path.starts_with('/')
        {
            return Err(RoutingError::InvalidDns);
        }
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), RoutingError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        Err(RoutingError::InvalidName)
    } else {
        Ok(())
    }
}

pub fn normalize_domain(host: &str) -> Result<String, RoutingError> {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return Err(RoutingError::InvalidDomain);
    }
    let normalized = host.to_ascii_lowercase();
    if normalized.split('.').any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err(RoutingError::InvalidDomain);
    }
    Ok(normalized)
}

pub fn domain_matches(host: &str, suffix: &str) -> bool {
    host == suffix
        || host
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn direct(table: u32, priority: u32, domains: &[&str], default_route: bool) -> ProviderConfig {
        ProviderConfig {
            target: Target::Device {
                stack_instance: "stack-a".into(),
                table,
                interface_id: 1,
            },
            ip_routes: vec![IpPrefix::new(IpAddress::V4([0, 0, 0, 0]), 0).unwrap()],
            domain_routes: domains.iter().map(|value| value.to_string()).collect(),
            dns_upstreams: Vec::new(),
            priority,
            default_route,
        }
    }

    #[test]
    fn suffixes_use_label_boundaries_and_longest_match() {
        let mut routes = Registry::new(8);
        let public = routes
            .register("public", direct(1, 100, &[], true))
            .unwrap();
        let corp = routes
            .register("corp", direct(2, 20, &["corp.test"], false))
            .unwrap();
        let dev = routes
            .register("dev", direct(3, 20, &["dev.corp.test"], false))
            .unwrap();
        assert_eq!(
            routes
                .open_flow(Destination::Domain {
                    host: "API.Dev.Corp.Test.".into(),
                    port: 443
                })
                .unwrap()
                .provider_token,
            dev
        );
        assert_eq!(
            routes
                .open_flow(Destination::Domain {
                    host: "notcorp.test".into(),
                    port: 443
                })
                .unwrap()
                .provider_token,
            public
        );
        assert_ne!(corp, public);
    }

    #[test]
    fn removal_is_fail_closed_and_pinned_flows_drain() {
        let mut routes = Registry::new(4);
        routes.register("public", direct(1, 10, &[], true)).unwrap();
        let corp = routes
            .register("corp", direct(2, 1, &["corp.test"], false))
            .unwrap();
        let lease = routes
            .open_flow(Destination::Domain {
                host: "db.corp.test".into(),
                port: 5432,
            })
            .unwrap();
        routes.remove(corp).unwrap();
        assert_eq!(
            routes.open_flow(Destination::Domain {
                host: "db.corp.test".into(),
                port: 5432
            }),
            Err(RoutingError::NetworkUnreachable)
        );
        assert_eq!(routes.remove_table(2), Err(RoutingError::ShouldWait));
        routes.close_flow(lease.flow_id).unwrap();
        assert!(routes.provider(corp).is_none());
    }

    #[test]
    fn cidr_uses_longest_prefix_then_priority() {
        let mut routes = Registry::new(4);
        routes
            .register("default", direct(1, 10, &[], true))
            .unwrap();
        let mut private = direct(2, 20, &[], false);
        private.ip_routes = vec![IpPrefix::new(IpAddress::V4([10, 0, 0, 0]), 8).unwrap()];
        routes.register("private", private).unwrap();
        let mut site = direct(3, 30, &[], false);
        site.ip_routes = vec![IpPrefix::new(IpAddress::V4([10, 4, 0, 0]), 16).unwrap()];
        let site = routes.register("site", site).unwrap();
        assert_eq!(
            routes
                .open_flow(Destination::Ip {
                    address: IpAddress::V4([10, 4, 2, 1]),
                    port: 80
                })
                .unwrap()
                .provider_token,
            site
        );
    }
}
