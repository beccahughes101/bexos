use crate::{Error, Result};
use bexos_userspace::Channel;
use std::sync::Arc;
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn get(
        &mut self,
        host: &str,
        path: &str,
        headers: &[(String, String)],
        maximum: usize,
    ) -> Result<Response>;
    async fn get_into(
        &mut self,
        host: &str,
        path: &str,
        headers: &[(String, String)],
        maximum: usize,
        sink: &mut impl FnMut(u16, &[u8]) -> Result<()>,
    ) -> Result<Response> {
        let mut response = self.get(host, path, headers, maximum).await?;
        sink(response.status, &response.body)?;
        response.body.clear();
        Ok(response)
    }
}
#[derive(Clone)]
pub struct LiveTransport {
    pub directory: Channel,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub tls: Arc<rustls::ClientConfig>,
    pub identity: Option<(String, Arc<rustls::ClientConfig>)>,
}
pub struct ClientIdentity(pub Arc<rustls::sign::CertifiedKey>);
impl core::fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("RegistryClientIdentity")
    }
}
impl rustls::client::ResolvesClientCert for ClientIdentity {
    fn resolve(
        &self,
        _: &[&[u8]],
        schemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.0.key.choose_scheme(schemes).map(|_| self.0.clone())
    }
    fn has_certs(&self) -> bool {
        true
    }
}
impl Transport for LiveTransport {
    async fn get(
        &mut self,
        host: &str,
        path: &str,
        headers: &[(String, String)],
        maximum: usize,
    ) -> Result<Response> {
        let mut body = Vec::new();
        let mut response = self
            .get_into(host, path, headers, maximum, &mut |_, chunk| {
                body.extend_from_slice(chunk);
                Ok(())
            })
            .await?;
        response.body = body;
        Ok(response)
    }
    async fn get_into(
        &mut self,
        host: &str,
        path: &str,
        headers: &[(String, String)],
        maximum: usize,
        sink: &mut impl FnMut(u16, &[u8]) -> Result<()>,
    ) -> Result<Response> {
        let (server_name, port) =
            bexos_pkg_client::split_registry_authority(host).ok_or(Error::InvalidArgs)?;
        let network =
            bexos_userspace::service_directory::ServiceDirectoryClient::new(self.directory)
                .connect("bexos.net.SocketProvider", "Public")
                .map_err(|error| unavailable("bind network provider", error))?;
        let stream = bexos_net::async_connect::connect_scoped_with_timeout(
            network,
            server_name,
            port,
            self.connect_timeout_ms,
        )
        .await
        .map_err(|error| network_error("connect TCP", error))?;
        let config = self
            .identity
            .as_ref()
            .filter(|(identity_host, _)| identity_host == host)
            .map_or_else(|| self.tls.clone(), |(_, config)| config.clone());
        let tls = bexos_net::async_http::tls_with_timeout(
            stream,
            server_name,
            config,
            self.connect_timeout_ms,
        )
        .await
        .map_err(|error| network_error("TLS handshake", error))?;
        let http2 = tls.conn.alpn_protocol() == Some(b"h2".as_slice());
        let mut received = 0usize;
        let response = bexos_net::async_http::get_into_with_timeout(
            tls,
            host,
            path,
            headers,
            maximum,
            http2,
            self.request_timeout_ms,
            &mut |status, bytes| {
                received = received.saturating_add(bytes.len());
                sink(status, bytes).map_err(|_| bexos_net::NetError::BodyTooLarge)
            },
        )
        .await;
        let response = response.map_err(|error| {
            #[cfg(feature = "guest")]
            bexos_userspace::log(&format!(
                "pkgd: incomplete HTTP response bytes={received} maximum={maximum}\n"
            ));
            network_error("HTTP request", error)
        })?;
        Ok(Response {
            status: response.status,
            headers: response.headers,
            body: response.body,
        })
    }
}

fn network_error(stage: &str, error: bexos_net::NetError) -> Error {
    let unavailable = unavailable(stage, &error);
    match error {
        bexos_net::NetError::TimedOut => Error::TimedOut,
        bexos_net::NetError::BodyTooLarge => Error::ResourceExhausted,
        _ => unavailable,
    }
}

fn unavailable(stage: &str, error: impl core::fmt::Debug) -> Error {
    // Only typed transport errors are recorded; URLs, headers and identity
    // material must never enter diagnostics.
    #[cfg(feature = "guest")]
    bexos_userspace::log(&format!("pkgd: {stage} failed: {error:?}\n"));
    #[cfg(not(feature = "guest"))]
    let _ = (stage, error);
    Error::Unavailable
}
