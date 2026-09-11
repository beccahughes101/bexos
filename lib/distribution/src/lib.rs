extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(feature = "live_https")]
use bexos_net::secure::{NetstackConnector, RootConfigCache};
#[cfg(feature = "live_https")]
use bexos_userspace::Channel;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistributionPolicy {
    pub deployable: bool,
    pub direct_web_installs: bool,
    pub allowed_tuf_origins: Vec<String>,
    pub max_metadata_bytes: usize,
    pub max_target_bytes: usize,
    pub max_redirects: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpsUrl {
    pub origin: String,
    pub host: String,
    pub path_and_query: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DistributionError {
    InvalidUrl,
    PolicyRejected,
    RedirectLimit,
    Downgrade,
    BodyTooLarge,
    BadHttp,
    Network,
    Tls,
    MissingService,
}

pub trait DistributionTransport {
    fn get(&mut self, url: &HttpsUrl, max_bytes: usize) -> Result<HttpResponse, DistributionError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[cfg(feature = "live_https")]
pub struct NetstackRustlsHttpsTransport {
    connector: NetstackConnector,
    roots: RootConfigCache,
}

#[cfg(feature = "live_https")]
impl NetstackRustlsHttpsTransport {
    pub fn new(netstack: Channel, tls_trust: Channel) -> Self {
        Self {
            connector: NetstackConnector::new(netstack),
            roots: RootConfigCache::new(tls_trust),
        }
    }
}

#[cfg(feature = "live_https")]
impl DistributionTransport for NetstackRustlsHttpsTransport {
    fn get(&mut self, url: &HttpsUrl, max_bytes: usize) -> Result<HttpResponse, DistributionError> {
        bexos_net::secure::https_get(
            &mut self.connector,
            &mut self.roots,
            &url.host,
            &url.path_and_query,
            max_bytes,
            "bexos-distribution/1",
        )
        .map(|response| HttpResponse {
            status: response.status,
            headers: response.headers,
            body: response.body,
        })
        .map_err(map_net_error)
    }
}

pub struct DistributionClient<T> {
    policy: DistributionPolicy,
    transport: T,
}

impl<T: DistributionTransport> DistributionClient<T> {
    pub fn new(policy: DistributionPolicy, transport: T) -> Self {
        Self { policy, transport }
    }

    pub fn fetch_direct_app(&mut self, url: &str) -> Result<Vec<u8>, DistributionError> {
        if !self.policy.deployable || !self.policy.direct_web_installs {
            return Err(DistributionError::PolicyRejected);
        }
        self.fetch_following_redirects(url, self.policy.max_target_bytes, false)
    }

    pub fn fetch_tuf_metadata(&mut self, url: &str) -> Result<Vec<u8>, DistributionError> {
        self.fetch_restricted(url, self.policy.max_metadata_bytes)
    }

    pub fn fetch_tuf_target(&mut self, url: &str) -> Result<Vec<u8>, DistributionError> {
        self.fetch_restricted(url, self.policy.max_target_bytes)
    }

    fn fetch_restricted(
        &mut self,
        url: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, DistributionError> {
        let parsed = parse_https_url(url)?;
        if !self
            .policy
            .allowed_tuf_origins
            .iter()
            .any(|origin| origin == &parsed.origin)
        {
            return Err(DistributionError::PolicyRejected);
        }
        self.fetch_following_redirects(url, max_bytes, true)
    }

    fn fetch_following_redirects(
        &mut self,
        url: &str,
        max_bytes: usize,
        restricted: bool,
    ) -> Result<Vec<u8>, DistributionError> {
        let mut current = parse_https_url(url)?;
        for redirect_count in 0..=self.policy.max_redirects {
            if restricted
                && !self
                    .policy
                    .allowed_tuf_origins
                    .iter()
                    .any(|origin| origin == &current.origin)
            {
                return Err(DistributionError::PolicyRejected);
            }
            let response = self.transport.get(&current, max_bytes)?;
            match response.status {
                200..=299 => {
                    if response.body.len() > max_bytes {
                        return Err(DistributionError::BodyTooLarge);
                    }
                    return Ok(response.body);
                }
                301 | 302 | 303 | 307 | 308 => {
                    if redirect_count == self.policy.max_redirects {
                        return Err(DistributionError::RedirectLimit);
                    }
                    let location = response
                        .headers
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                        .map(|(_, value)| value.as_str())
                        .ok_or(DistributionError::BadHttp)?;
                    let next =
                        parse_https_url(location).map_err(|_| DistributionError::Downgrade)?;
                    if next.origin.starts_with("http://") {
                        return Err(DistributionError::Downgrade);
                    }
                    current = next;
                }
                _ => return Err(DistributionError::Network),
            }
        }
        Err(DistributionError::RedirectLimit)
    }
}

pub fn parse_https_url(url: &str) -> Result<HttpsUrl, DistributionError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or(DistributionError::InvalidUrl)?;
    if rest.contains('#') || rest.contains('@') || rest.is_empty() {
        return Err(DistributionError::InvalidUrl);
    }
    let split = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..split];
    if authority.is_empty() || authority.contains(':') && !authority.ends_with(":443") {
        return Err(DistributionError::InvalidUrl);
    }
    let host = authority.strip_suffix(":443").unwrap_or(authority);
    if host.is_empty()
        || host.starts_with('.')
        || host.ends_with('.')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err(DistributionError::InvalidUrl);
    }
    let path_and_query = if split == rest.len() {
        "/"
    } else {
        &rest[split..]
    };
    Ok(HttpsUrl {
        origin: format!("https://{host}"),
        host: host.to_string(),
        path_and_query: path_and_query.to_string(),
    })
}

pub fn parse_http1_response(bytes: &[u8], cap: usize) -> Result<HttpResponse, DistributionError> {
    bexos_net::http1::parse_response(bytes, cap)
        .map(|response| HttpResponse {
            status: response.status,
            headers: response.headers,
            body: response.body,
        })
        .map_err(map_net_error)
}

fn map_net_error(error: bexos_net::NetError) -> DistributionError {
    match error {
        bexos_net::NetError::InvalidArgs => DistributionError::InvalidUrl,
        bexos_net::NetError::BodyTooLarge => DistributionError::BodyTooLarge,
        bexos_net::NetError::BadHttp => DistributionError::BadHttp,
        bexos_net::NetError::Tls => DistributionError::Tls,
        bexos_net::NetError::MissingService => DistributionError::MissingService,
        _ => DistributionError::Network,
    }
}
