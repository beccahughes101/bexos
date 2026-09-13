mod server;
use server::{Reply, Server, client_config, drive};
use std::{net::TcpStream, sync::Arc};

struct BlockedIo(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for BlockedIo {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl std::io::Read for BlockedIo {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::WouldBlock.into())
    }
}
impl std::io::Write for BlockedIo {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::WouldBlock.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::ErrorKind::WouldBlock.into())
    }
}

#[test]
fn configured_deadlines_and_unpolled_cancellation_close_streams() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let closed = Arc::new(AtomicUsize::new(0));
    let unpolled = bexos_net::async_http::tls_with_timeout(
        BlockedIo(closed.clone()),
        "localhost",
        client_config(false),
        5,
    );
    drop(unpolled);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    assert!(matches!(
        drive(bexos_net::async_http::tls_with_timeout(
            BlockedIo(closed.clone()),
            "localhost",
            client_config(false),
            5,
        )),
        Err(bexos_net::NetError::TimedOut)
    ));
    assert_eq!(closed.load(Ordering::SeqCst), 2);
    assert!(matches!(
        drive(bexos_net::async_http::get_into_with_timeout(
            BlockedIo(closed.clone()),
            "localhost",
            "/",
            &[],
            1,
            false,
            5,
            &mut |_, _| panic!("blocked transport cannot produce response bytes"),
        )),
        Err(bexos_net::NetError::TimedOut)
    ));
    assert_eq!(closed.load(Ordering::SeqCst), 3);
}

struct PartialSocket {
    socket: TcpStream,
    read_blocked: bool,
    write_blocked: bool,
}
impl std::io::Read for PartialSocket {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.read_blocked = !self.read_blocked;
        if self.read_blocked {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        let maximum = bytes.len().min(4096);
        std::io::Read::read(&mut self.socket, &mut bytes[..maximum])
    }
}
impl std::io::Write for PartialSocket {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.write_blocked = !self.write_blocked;
        if self.write_blocked {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        std::io::Write::write(&mut self.socket, &bytes[..bytes.len().min(1024)])
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.socket)
    }
}

#[test]
fn h2_payload_crosses_flow_windows_with_partial_io_and_server_shutdown() {
    let expected: Vec<_> = (0..512 * 1024).map(|index| (index % 251) as u8).collect();
    let payload = expected.clone();
    let server = Server::start(true, true, move |path, _| {
        assert_eq!(path, "/large");
        Reply {
            status: 200,
            headers: vec![],
            body: payload.clone(),
        }
    });
    for _ in 0..2 {
        let socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, server.port)).unwrap();
        socket.set_nonblocking(true).unwrap();
        socket.set_nodelay(true).unwrap();
        let socket = PartialSocket {
            socket,
            read_blocked: false,
            write_blocked: false,
        };
        let mut bytes = Vec::new();
        drive(async {
            let tls = bexos_net::async_http::tls(socket, "localhost", client_config(true))
                .await
                .unwrap();
            let response = bexos_net::async_http::get_into(
                tls,
                &format!("localhost:{}", server.port),
                "/large",
                &[],
                expected.len(),
                true,
                &mut |status, chunk| {
                    assert_eq!(status, 200);
                    bytes.extend_from_slice(chunk);
                    Ok(())
                },
            )
            .await
            .unwrap();
            assert_eq!(response.status, 200);
        });
        assert_eq!(bytes, expected);
    }
}

#[test]
fn real_https_streaming_negotiates_h2_h1_and_mtls() {
    for (http2, mtls) in [(true, false), (false, false), (true, true)] {
        let server = Server::start(http2, mtls, |path, _| {
            assert_eq!(path, "/blob");
            Reply {
                status: 200,
                headers: vec![],
                body: b"verified".to_vec(),
            }
        });
        let socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, server.port)).unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut bytes = Vec::new();
        drive(async {
            let tls = bexos_net::async_http::tls(socket, "localhost", client_config(mtls))
                .await
                .unwrap();
            assert_eq!(
                tls.conn.alpn_protocol(),
                Some(if http2 {
                    b"h2".as_slice()
                } else {
                    b"http/1.1".as_slice()
                })
            );
            let response = bexos_net::async_http::get_into(
                tls,
                &format!("localhost:{}", server.port),
                "/blob",
                &[],
                8,
                http2,
                &mut |status, chunk| {
                    assert_eq!(status, 200);
                    bytes.extend_from_slice(chunk);
                    Ok(())
                },
            )
            .await
            .unwrap();
            assert_eq!(response.status, 200);
            assert!(response.body.is_empty());
        });
        assert_eq!(bytes, b"verified");
    }
}
#[derive(Clone)]
struct HostTransport {
    port: u16,
    tls: Arc<rustls::ClientConfig>,
}
impl bexos_pkgd::transport::Transport for HostTransport {
    async fn get(
        &mut self,
        host: &str,
        path: &str,
        headers: &[(String, String)],
        maximum: usize,
    ) -> bexos_pkgd::Result<bexos_pkgd::transport::Response> {
        let socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.port))
            .map_err(|_| bexos_pkgd::Error::Unavailable)?;
        socket.set_nonblocking(true).unwrap();
        let tls = bexos_net::async_http::tls(socket, "localhost", self.tls.clone())
            .await
            .map_err(|_| bexos_pkgd::Error::Unavailable)?;
        let response = bexos_net::async_http::get(tls, host, path, headers, maximum, true)
            .await
            .map_err(|_| bexos_pkgd::Error::Unavailable)?;
        Ok(bexos_pkgd::transport::Response {
            status: response.status,
            headers: response.headers,
            body: response.body,
        })
    }
}
#[test]
fn signed_oci_over_real_h2_bearer_and_mtls_then_offline_cache() {
    use bexos_pkgd::{
        cas::Cas,
        oci::Oci,
        resolution::{resolve, resolve_cached},
    };
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
    let mut fixture = pkg_test_fixture::fixture();
    let responses = fixture.transport.responses;
    let port = Arc::new(AtomicU16::new(0));
    let issued = Arc::new(AtomicUsize::new(0));
    let issuer_count = issued.clone();
    let server_port = port.clone();
    let server = Server::start(true, true, move |path, headers| {
        if path.starts_with("/token?") {
            assert!(path.contains("scope=repository%3Aapps%2Fdemo%3Apull"));
            assert!(headers.iter().all(|(k, _)| k != "authorization"));
            issuer_count.fetch_add(1, Ordering::SeqCst);
            return Reply {
                status: 200,
                headers: vec![],
                body: br#"{"token":"fixture-pull"}"#.to_vec(),
            };
        }
        if !headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer fixture-pull")
        {
            return Reply {
                status: 401,
                headers: vec![(
                    "www-authenticate".into(),
                    format!(
                        "Bearer realm=\"https://localhost:{}/token\",scope=\"repository:apps/demo:pull\"",
                        server_port.load(Ordering::Acquire)
                    ),
                )],
                body: vec![],
            };
        }
        match responses.get(path) {
            Some(body) => Reply {
                status: 200,
                headers: vec![],
                body: body.clone(),
            },
            None => Reply {
                status: 404,
                headers: vec![],
                body: vec![],
            },
        }
    });
    port.store(server.port, Ordering::Release);
    let authority = format!("localhost:{}", server.port);
    fixture.config.repositories[0].host = authority.clone();
    fixture.config.repositories[0].token_origins = vec![format!("https://{authority}")];
    fixture.config.consumers[0].repositories = vec![fixture.config.repositories[0].id()];
    fixture.query.registry_host = authority;
    let mut oci = Oci {
        repository: fixture.config.repositories[0].clone(),
        transport: HostTransport {
            port: server.port,
            tls: client_config(true),
        },
        token: None,
        transfers: Default::default(),
        credential_generation: 1,
    };
    let mut secure = pkg_test_fixture::TestStore::default();
    let mut cas = Cas::open(
        bexos_redb::create_with_store(bexos_redb::mem::MemBlockStore::new()).unwrap(),
        1024 * 1024,
    )
    .unwrap();
    let result = drive(resolve(
        &fixture.config,
        "appd",
        &fixture.query,
        1_800_000_000,
        &mut oci,
        &mut secure,
        &mut cas,
    ))
    .unwrap();
    assert_eq!(&**result.bytes, fixture.payload.as_slice());
    assert_eq!(issued.load(Ordering::SeqCst), 1);
    drop(server);
    assert_eq!(
        &**resolve_cached(
            &fixture.config,
            "appd",
            &result.digest,
            &mut secure,
            &mut cas
        )
        .unwrap()
        .bytes,
        fixture.payload.as_slice()
    );
}
