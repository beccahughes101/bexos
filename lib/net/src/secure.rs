use alloc::string::ToString;
use alloc::vec::Vec;
use std::io::{Read, Write};
use std::sync::Arc;

use bexos_userspace::{Channel, Memory, Rpc, Socket};
use net_fidl::{
    HandleRef as NetHandleRef, IpAddress, NetstackConnectTcpRequest, NetstackResolveHostRequest,
    SocketAddress, SocketOptions, Status as NetStatus, TcpSocketGetStreamRequest,
};
use rustls::pki_types::{CertificateDer, ServerName};
use trust_fidl::{
    HandleRef as TrustHandleRef, TlsRootBundleFormat, TlsTrustManagerGetTlsRootBundleRequest,
    TlsTrustManagerGetTlsRootBundleResponse, TrustStatus,
};

use crate::NetError;
use crate::http1;

const IO_CHUNK: usize = 8192;
const HANDSHAKE_DEADLINE_MS: u64 = 10_000;
const REQUEST_DEADLINE_MS: u64 = 10_000;
const RESPONSE_DEADLINE_MS: u64 = 30_000;
const READ_OVERHEAD_CAP: usize = 8192;

#[derive(Clone)]
pub struct RootConfigCache {
    tls_trust: Channel,
    cached: Option<(u64, Arc<rustls::ClientConfig>)>,
}

impl RootConfigCache {
    pub fn new(tls_trust: Channel) -> Self {
        Self {
            tls_trust,
            cached: None,
        }
    }

    pub fn config(&mut self, alpn: &[&[u8]]) -> Result<Arc<rustls::ClientConfig>, NetError> {
        let (generation, roots) = fetch_tls_root_bundle(self.tls_trust)?;
        if let Some((cached_generation, config)) = &self.cached {
            if *cached_generation == generation && alpn_matches(config, alpn) {
                return Ok(config.clone());
            }
        }
        let mut store = rustls::RootCertStore::empty();
        for root in roots {
            store
                .add(CertificateDer::from(root))
                .map_err(|_| NetError::Tls)?;
        }
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(store)
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let config = Arc::new(config);
        self.cached = Some((generation, config.clone()));
        Ok(config)
    }
}

fn alpn_matches(config: &rustls::ClientConfig, alpn: &[&[u8]]) -> bool {
    config.alpn_protocols.len() == alpn.len()
        && config
            .alpn_protocols
            .iter()
            .zip(alpn.iter())
            .all(|(left, right)| left.as_slice() == *right)
}

pub trait TlsConnector {
    type Stream: Read + Write;

    fn connect(&mut self, host: &str, port: u16) -> Result<Self::Stream, NetError>;
}

#[derive(Clone, Copy)]
pub struct NetstackConnector {
    netstack: Channel,
}

impl NetstackConnector {
    pub fn new(netstack: Channel) -> Self {
        Self { netstack }
    }
}

impl TlsConnector for NetstackConnector {
    type Stream = BexosSocketIo;

    fn connect(&mut self, host: &str, port: u16) -> Result<Self::Stream, NetError> {
        let socket = connect_tcp(self.netstack, host, port)?;
        Ok(BexosSocketIo::new(socket))
    }
}

pub fn connect_tls<S: Read + Write>(
    transport: S,
    host: &str,
    config: Arc<rustls::ClientConfig>,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, S>, NetError> {
    let server_name = ServerName::try_from(host.to_string()).map_err(|_| NetError::InvalidArgs)?;
    let mut conn = rustls::ClientConnection::new(config, server_name).map_err(|_| NetError::Tls)?;
    let mut transport = transport;
    let deadline = Deadline::after(HANDSHAKE_DEADLINE_MS);
    while conn.is_handshaking() {
        match conn.complete_io(&mut transport) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if deadline.expired() {
                    return Err(NetError::TimedOut);
                }
                bexos_userspace::yield_now();
            }
            Err(_) => return Err(NetError::Tls),
        }
    }
    Ok(rustls::StreamOwned::new(conn, transport))
}

pub fn https_get<C: TlsConnector>(
    connector: &mut C,
    roots: &mut RootConfigCache,
    host: &str,
    path_and_query: &str,
    max_bytes: usize,
    user_agent: &str,
) -> Result<http1::Response, NetError> {
    let transport = connector.connect(host, 443)?;
    let mut tls = connect_tls(transport, host, roots.config(&[])?)?;
    let mut request = Vec::new();
    http1::encode_get_with_user_agent(host, path_and_query, user_agent, &mut request)?;
    write_all_deadlined(&mut tls, &request, REQUEST_DEADLINE_MS)?;
    let bytes = read_to_close(&mut tls, max_bytes, RESPONSE_DEADLINE_MS)?;
    http1::parse_response(&bytes, max_bytes)
}

pub fn tls_exchange<S: Read + Write>(
    transport: S,
    roots: &mut RootConfigCache,
    host: &str,
    alpn: &[&[u8]],
    request: &[u8],
    max_response_bytes: usize,
) -> Result<Vec<u8>, NetError> {
    let mut tls = connect_tls(transport, host, roots.config(alpn)?)?;
    write_all_deadlined(&mut tls, request, REQUEST_DEADLINE_MS)?;
    read_to_close(&mut tls, max_response_bytes, RESPONSE_DEADLINE_MS)
}

pub fn tls_exchange_with_exporters<S: Read + Write>(
    transport: S,
    roots: &mut RootConfigCache,
    host: &str,
    alpn: &[&[u8]],
    request: &[u8],
    max_response_bytes: usize,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), NetError> {
    let mut tls = connect_tls(transport, host, roots.config(alpn)?)?;
    write_all_deadlined(&mut tls, request, REQUEST_DEADLINE_MS)?;
    let mut c2s = vec![0u8; 32];
    let mut s2c = vec![0u8; 32];
    tls.conn
        .export_keying_material(
            &mut c2s,
            b"EXPORTER-network-time-security",
            Some(&[0, 0, 0, 15, 0]),
        )
        .map_err(|_| NetError::Tls)?;
    tls.conn
        .export_keying_material(
            &mut s2c,
            b"EXPORTER-network-time-security",
            Some(&[0, 0, 0, 15, 1]),
        )
        .map_err(|_| NetError::Tls)?;
    let response = read_to_close(&mut tls, max_response_bytes, RESPONSE_DEADLINE_MS)?;
    Ok((response, c2s, s2c))
}

fn write_all_deadlined<W: Write>(
    writer: &mut W,
    mut bytes: &[u8],
    ms: u64,
) -> Result<(), NetError> {
    let deadline = Deadline::after(ms);
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => {
                if deadline.expired() {
                    return Err(NetError::TimedOut);
                }
                bexos_userspace::yield_now();
            }
            Ok(written) => bytes = &bytes[written..],
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if deadline.expired() {
                    return Err(NetError::TimedOut);
                }
                bexos_userspace::yield_now();
            }
            Err(_) => return Err(NetError::Network),
        }
    }
    writer.flush().map_err(|_| NetError::Network)
}

fn read_to_close<R: Read>(reader: &mut R, max_bytes: usize, ms: u64) -> Result<Vec<u8>, NetError> {
    let deadline = Deadline::after(ms);
    let mut bytes = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(bytes),
            Ok(read) => {
                bytes.extend_from_slice(&buf[..read]);
                if bytes.len() > max_bytes.saturating_add(READ_OVERHEAD_CAP) {
                    return Err(NetError::BodyTooLarge);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if deadline.expired() {
                    return Err(NetError::TimedOut);
                }
                bexos_userspace::yield_now();
            }
            Err(_) => return Err(NetError::Network),
        }
    }
}

pub struct BexosSocketIo {
    socket: Socket,
    nonblocking: bool,
    control: Option<Channel>,
}

impl BexosSocketIo {
    pub fn new(socket: Socket) -> Self {
        Self {
            socket,
            nonblocking: false,
            control: None,
        }
    }
    pub fn with_control(socket: Socket, control: Channel) -> Self {
        Self {
            socket,
            control: Some(control),
            nonblocking: true,
        }
    }
    pub fn nonblocking(mut self) -> Self {
        self.nonblocking = true;
        self
    }
}

impl Drop for BexosSocketIo {
    fn drop(&mut self) {
        let _ = Memory::close(self.socket.0);
        if let Some(control) = self.control.take() {
            let _ = Memory::close(control.0);
        }
    }
}

impl Read for BexosSocketIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = Deadline::after(RESPONSE_DEADLINE_MS);
        loop {
            match self.socket.info() {
                Ok(info) if info.readable_bytes == 0 && info.peer_write_closed => return Ok(0),
                Ok(info) if info.readable_bytes == 0 => {
                    if self.nonblocking {
                        return Err(would_block());
                    }
                    if deadline.expired() {
                        return Err(would_block());
                    }
                    bexos_userspace::yield_now();
                    continue;
                }
                Ok(_) => {}
                Err(kernel_fidl::Status::ErrTimedOut) => return Err(would_block()),
                Err(_) => return Err(connection_aborted()),
            }
            let max = u32::try_from(buf.len())
                .unwrap_or(u32::MAX)
                .min(IO_CHUNK as u32);
            match self.socket.read(max) {
                Ok(bytes) => {
                    let len = bytes.len().min(buf.len());
                    buf[..len].copy_from_slice(&bytes[..len]);
                    return Ok(len);
                }
                Err(kernel_fidl::Status::ErrTimedOut) => return Err(would_block()),
                Err(kernel_fidl::Status::ErrPeerClosed) => return Ok(0),
                Err(_) => return Err(connection_aborted()),
            }
        }
    }
}

impl Write for BexosSocketIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(IO_CHUNK);
        match self.socket.write(&buf[..len]) {
            Ok(0) => Err(would_block()),
            Ok(written) => Ok(written as usize),
            Err(kernel_fidl::Status::ErrTimedOut) => Err(would_block()),
            Err(_) => Err(connection_aborted()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn would_block() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::WouldBlock)
}

fn connection_aborted() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::ConnectionAborted)
}

fn connect_tcp(netstack: Channel, host: &str, port: u16) -> Result<Socket, NetError> {
    let mut client = net_fidl::NetstackPublicClient::new(Rpc(netstack));
    let mut req = [0; 512];
    let mut resp = [0; 512];
    let mut req_handles = [NetHandleRef { raw: 0 }; 2];
    let mut resp_handles = [NetHandleRef { raw: 0 }; 2];
    let resolved = client
        .resolve_host(
            &NetstackResolveHostRequest { hostname: host },
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| NetError::Network)?;
    if resolved.status != NetStatus::Ok {
        return Err(NetError::Network);
    }
    let address = resolved.addresses.get(0).map_err(|_| NetError::Network)?;
    connect_tcp_addr(netstack, address, port)
}

pub fn connect_tcp_addr(
    netstack: Channel,
    address: IpAddress,
    port: u16,
) -> Result<Socket, NetError> {
    let mut client = net_fidl::NetstackPublicClient::new(Rpc(netstack));
    let mut req = [0; 512];
    let mut resp = [0; 512];
    let mut req_handles = [NetHandleRef { raw: 0 }; 2];
    let mut resp_handles = [NetHandleRef { raw: 0 }; 2];
    let (client_end, server_end) = Channel::pair().map_err(|_| NetError::Network)?;
    let request = NetstackConnectTcpRequest {
        remote_addr: SocketAddress {
            addr: address,
            port,
        },
        options: SocketOptions {
            non_blocking: Some(false),
            keep_alive_ms: Some(0),
            rx_buffer_size: Some(64 * 1024),
            tx_buffer_size: Some(64 * 1024),
        },
        socket: NetHandleRef { raw: server_end.0 },
    };
    let connected = client
        .connect_tcp(
            &request,
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| NetError::Network)?;
    if connected.status != NetStatus::Ok {
        return Err(NetError::Network);
    }
    tcp_get_stream(client_end)
}

fn tcp_get_stream(control: Channel) -> Result<Socket, NetError> {
    let mut client = net_fidl::TcpSocketPublicClient::new(Rpc(control));
    let mut req = [0; 128];
    let mut resp = [0; 128];
    let mut req_handles = [NetHandleRef { raw: 0 }; 1];
    let mut resp_handles = [NetHandleRef { raw: 0 }; 1];
    let stream = client
        .get_stream(
            &TcpSocketGetStreamRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| NetError::Network)?;
    if stream.status == NetStatus::Ok {
        Ok(Socket(stream.socket.raw))
    } else {
        Err(NetError::Network)
    }
}

pub fn fetch_tls_root_bundle(tls_trust: Channel) -> Result<(u64, Vec<Vec<u8>>), NetError> {
    let mut client = trust_fidl::TlsTrustManagerPublicClient::new(Rpc(tls_trust));
    let mut req = [0; 128];
    let mut resp = [0; 256];
    let mut req_handles = [TrustHandleRef { raw: 0 }; 1];
    let mut resp_handles = [TrustHandleRef { raw: 0 }; 1];
    let response: TlsTrustManagerGetTlsRootBundleResponse = client
        .get_tls_root_bundle(
            &TlsTrustManagerGetTlsRootBundleRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| NetError::MissingService)?;
    if response.status != TrustStatus::Ok || response.format != TlsRootBundleFormat::RedbTlsRoots {
        return Err(NetError::MissingService);
    }
    let len = bexos_boot::page_round(response.length).ok_or(NetError::Network)?;
    let va = Memory::map(response.bundle.raw, len, 2).map_err(|_| NetError::Network)?;
    let bytes =
        unsafe { core::slice::from_raw_parts(va as *const u8, response.length as usize) }.to_vec();
    Memory::unmap(va, len).map_err(|_| NetError::Network)?;
    let store = bexos_redb::mem::MemBlockStore::from_bytes(bytes);
    let db = bexos_trust_store::persistent::TrustStoreDb::open_with_backend(
        bexos_redb::RedbStorageBackend::new(store),
    )
    .map_err(|_| NetError::Network)?;
    let roots = db
        .list_tls_roots()
        .map_err(|_| NetError::Network)?
        .into_iter()
        .map(|root| root.der_bytes)
        .collect();
    Ok((response.generation, roots))
}

#[derive(Clone, Copy)]
struct Deadline {
    end_ticks: u64,
}

impl Deadline {
    fn after(ms: u64) -> Self {
        let ticks = bexos_userspace::syscall::ticks();
        let hz = bexos_userspace::syscall::frequency().max(1);
        let delta = ms.saturating_mul(hz) / 1000;
        Self {
            end_ticks: ticks.saturating_add(delta),
        }
    }

    fn expired(self) -> bool {
        let now = bexos_userspace::syscall::ticks();
        now >= self.end_ticks
    }
}
