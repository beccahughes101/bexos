//! Test-owned loopback TLS listener. Drop stops and joins every worker.
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::{
    future::Future,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};
pub const ROOT: &[u8] = include_bytes!(env!("ROOT_CERT"));
const SERVER: &[u8] = include_bytes!(env!("SERVER_CERT"));
const SERVER_KEY: &[u8] = include_bytes!(env!("SERVER_KEY"));
const CLIENT: &[u8] = include_bytes!(env!("CLIENT_CERT"));
const CLIENT_KEY: &[u8] = include_bytes!(env!("CLIENT_KEY"));
pub fn drive<F: Future>(future: F) -> F::Output {
    drive_for(future, Duration::from_secs(60))
}
fn drive_for<F: Future>(future: F, timeout: Duration) -> F::Output {
    let start = Instant::now();
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                assert!(start.elapsed() < timeout, "HTTPS fixture timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
pub fn roots() -> Arc<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(ROOT.to_vec())).unwrap();
    Arc::new(roots)
}
pub fn client_config(mtls: bool) -> Arc<rustls::ClientConfig> {
    let builder = rustls::ClientConfig::builder().with_root_certificates(roots());
    let mut config = if mtls {
        builder
            .with_client_auth_cert(
                vec![CertificateDer::from(CLIENT.to_vec())],
                PrivateKeyDer::try_from(CLIENT_KEY.to_vec()).unwrap(),
            )
            .unwrap()
    } else {
        builder.with_no_client_auth()
    };
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Arc::new(config)
}
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
pub struct Server {
    pub port: u16,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.expect("HTTPS fixture server failed");
            }
        }
    }
}
impl Server {
    pub fn start(
        http2: bool,
        mtls: bool,
        handler: impl Fn(&str, &[(String, String)]) -> Reply + Send + 'static,
    ) -> Self {
        Self::start_port(0, http2, mtls, handler)
    }
    pub fn start_port(
        port: u16,
        http2: bool,
        mtls: bool,
        handler: impl Fn(&str, &[(String, String)]) -> Reply + Send + 'static,
    ) -> Self {
        Self::start_port_with_timeout(port, http2, mtls, Duration::from_secs(60), handler)
    }
    pub fn start_port_with_timeout(
        port: u16,
        http2: bool,
        mtls: bool,
        timeout: Duration,
        handler: impl Fn(&str, &[(String, String)]) -> Reply + Send + 'static,
    ) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            let builder = rustls::ServerConfig::builder();
            let builder = if mtls {
                builder.with_client_cert_verifier(
                    rustls::server::WebPkiClientVerifier::builder(roots())
                        .build()
                        .unwrap(),
                )
            } else {
                builder.with_no_client_auth()
            };
            let mut config = builder
                .with_single_cert(
                    vec![CertificateDer::from(SERVER.to_vec())],
                    PrivateKeyDer::try_from(SERVER_KEY.to_vec()).unwrap(),
                )
                .unwrap();
            config.alpn_protocols = vec![if http2 {
                b"h2".to_vec()
            } else {
                b"http/1.1".to_vec()
            }];
            let config = Arc::new(config);
            while !stopped.load(Ordering::Acquire) {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(e) => panic!("listener: {e}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut stream = rustls::StreamOwned::new(
                    rustls::ServerConnection::new(config.clone()).unwrap(),
                    socket,
                );
                while stream.conn.is_handshaking() {
                    if stream.conn.complete_io(&mut stream.sock).is_err() {
                        break;
                    }
                }
                if stream.conn.is_handshaking() {
                    continue;
                }
                if http2 {
                    stream.sock.set_nonblocking(true).unwrap();
                    drive_for(
                        async {
                            let Ok(mut connection) =
                                h2::server::handshake(bexos_net::async_http::Io(stream)).await
                            else {
                                return;
                            };
                            let Some(Ok((request, mut respond))) = connection.accept().await else {
                                return;
                            };
                            let headers: Vec<_> = request
                                .headers()
                                .iter()
                                .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
                                .collect();
                            let reply =
                                handler(request.uri().path_and_query().unwrap().as_str(), &headers);
                            let mut response = http::Response::builder()
                                .status(reply.status)
                                .header("content-length", reply.body.len());
                            for (k, v) in reply.headers {
                                response = response.header(k, v);
                            }
                            let Ok(mut body) =
                                respond.send_response(response.body(()).unwrap(), false)
                            else {
                                return;
                            };
                            if body.send_data(reply.body.into(), true).is_err() {
                                return;
                            }
                            // This fixture serves one request per connection. Finish
                            // that response and close with GOAWAY instead of holding
                            // the listener until a client's TCP teardown arrives.
                            connection.graceful_shutdown();
                            let _ = std::future::poll_fn(|cx| connection.poll_closed(cx)).await;
                        },
                        timeout,
                    );
                } else {
                    let mut head = Vec::new();
                    let mut byte = [0; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if stream.read_exact(&mut byte).is_err() {
                            break;
                        }
                        head.push(byte[0]);
                        assert!(head.len() < 8192);
                    }
                    let text = String::from_utf8(head).unwrap();
                    let mut lines = text.split("\r\n");
                    let Some(path) = lines.next().and_then(|l| l.split_whitespace().nth(1)) else {
                        continue;
                    };
                    let headers: Vec<_> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    let reply = handler(path, &headers);
                    let mut head = format!(
                        "HTTP/1.1 {} Response\r\nTransfer-Encoding: chunked\r\n",
                        reply.status
                    );
                    for (k, v) in reply.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    stream.write_all(head.as_bytes()).unwrap();
                    for chunk in reply.body.chunks(3) {
                        stream
                            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                            .unwrap();
                        stream.write_all(chunk).unwrap();
                        stream.write_all(b"\r\n").unwrap();
                    }
                    stream.write_all(b"0\r\n\r\n").unwrap();
                    stream.flush().unwrap();
                }
            }
        });
        Self {
            port,
            stop,
            worker: Some(worker),
        }
    }
}
