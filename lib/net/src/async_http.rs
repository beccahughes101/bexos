//! HTTP over caller-owned nonblocking TLS streams; no Tokio runtime or OS sockets.
use crate::{NetError, http1::Response};
use std::{
    future::{Future, poll_fn},
    io::{self, Read, Write},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct Io<S>(pub S);
impl<S: Read + Unpin> AsyncRead for Io<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.0.read(out.initialize_unfilled()) {
            Ok(n) => {
                out.advance(n);
                Poll::Ready(Ok(()))
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}
impl<S: Write + Unpin> AsyncWrite for Io<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.0.write(bytes) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            result => Poll::Ready(result),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.0.flush() {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            result => Poll::Ready(result),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

pub async fn with_deadline<T>(
    future: impl Future<Output = Result<T, NetError>>,
    timeout_ms: u64,
) -> Result<T, NetError> {
    #[cfg(bexos_guest)]
    let end = bexos_userspace::live_migration::now_ms().saturating_add(timeout_ms);
    #[cfg(not(bexos_guest))]
    let start = std::time::Instant::now();
    let mut future = core::pin::pin!(future);
    poll_fn(|cx| {
        #[cfg(bexos_guest)]
        let expired = bexos_userspace::live_migration::now_ms() >= end;
        #[cfg(not(bexos_guest))]
        let expired = start.elapsed().as_millis() >= timeout_ms as u128;
        if expired {
            return Poll::Ready(Err(NetError::TimedOut));
        }
        future.as_mut().poll(cx)
    })
    .await
}

pub async fn tls<S: Read + Write + Unpin>(
    stream: S,
    host: &str,
    config: Arc<rustls::ClientConfig>,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, S>, NetError> {
    tls_with_timeout(stream, host, config, 10_000).await
}

pub async fn tls_with_timeout<S: Read + Write + Unpin>(
    mut stream: S,
    host: &str,
    config: Arc<rustls::ClientConfig>,
    timeout_ms: u64,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, S>, NetError> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| NetError::InvalidArgs)?;
    let mut connection = rustls::ClientConnection::new(config, name).map_err(|_| NetError::Tls)?;
    with_deadline(
        poll_fn(|cx| {
            if !connection.is_handshaking() {
                return Poll::Ready(Ok(()));
            }
            match connection.complete_io(&mut stream) {
                Ok(_) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Err(_) => Poll::Ready(Err(NetError::Tls)),
            }
        }),
        timeout_ms,
    )
    .await?;
    Ok(rustls::StreamOwned::new(connection, stream))
}

pub async fn get<S: Read + Write + Unpin>(
    stream: S,
    host: &str,
    path: &str,
    headers: &[(String, String)],
    maximum: usize,
    http2: bool,
) -> Result<Response, NetError> {
    let mut body = Vec::new();
    let mut response = get_into(
        stream,
        host,
        path,
        headers,
        maximum,
        http2,
        &mut |_, chunk| {
            body.extend_from_slice(chunk);
            Ok(())
        },
    )
    .await?;
    response.body = body;
    Ok(response)
}

pub async fn get_into<S: Read + Write + Unpin>(
    stream: S,
    host: &str,
    path: &str,
    headers: &[(String, String)],
    maximum: usize,
    http2: bool,
    sink: &mut impl FnMut(u16, &[u8]) -> Result<(), NetError>,
) -> Result<Response, NetError> {
    get_into_with_timeout(stream, host, path, headers, maximum, http2, 30_000, sink).await
}

pub async fn get_into_with_timeout<S: Read + Write + Unpin>(
    stream: S,
    host: &str,
    path: &str,
    headers: &[(String, String)],
    maximum: usize,
    http2: bool,
    timeout_ms: u64,
    sink: &mut impl FnMut(u16, &[u8]) -> Result<(), NetError>,
) -> Result<Response, NetError> {
    if host.is_empty()
        || host.len() > 255
        || host
            .bytes()
            .any(|b| b <= 32 || b >= 127 || b"/\\?#@".contains(&b))
        || !path.starts_with('/')
        || path.bytes().any(|b| b <= 32 || b == 127)
    {
        return Err(NetError::InvalidArgs);
    }
    for (key, value) in headers {
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 32 || b == 127)
        {
            return Err(NetError::InvalidArgs);
        }
    }
    with_deadline(
        async {
            if http2 {
                get_h2(stream, host, path, headers, maximum, sink).await
            } else {
                get_h1(stream, host, path, headers, maximum, sink).await
            }
        },
        timeout_ms,
    )
    .await
}

async fn get_h2<S: Read + Write + Unpin>(
    stream: S,
    host: &str,
    path: &str,
    headers: &[(String, String)],
    maximum: usize,
    sink: &mut impl FnMut(u16, &[u8]) -> Result<(), NetError>,
) -> Result<Response, NetError> {
    let (mut sender, connection) = h2::client::Builder::new()
        .max_header_list_size(16 * 1024)
        .handshake::<_, &'static [u8]>(Io(stream))
        .await
        .map_err(|_| NetError::BadHttp)?;
    let work = async {
        let mut request = http::Request::builder()
            .method("GET")
            .uri(format!("https://{host}{path}"));
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let request = request.body(()).map_err(|_| NetError::InvalidArgs)?;
        let (response, _) = sender
            .send_request(request, true)
            .map_err(|_| NetError::Network)?;
        let response = response.await.map_err(|_| NetError::Network)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.to_string(),
                    value.to_str().map_err(|_| NetError::BadHttp)?.to_string(),
                ))
            })
            .collect::<Result<Vec<_>, NetError>>()?;
        let mut stream = response.into_body();
        let mut total = 0usize;
        while let Some(chunk) = stream.data().await {
            let chunk = chunk.map_err(|_| NetError::Network)?;
            if chunk.len() > maximum.saturating_sub(total) {
                return Err(NetError::BodyTooLarge);
            }
            sink(status, &chunk)?;
            total += chunk.len();
            stream
                .flow_control()
                .release_capacity(chunk.len())
                .map_err(|_| NetError::BadHttp)?;
        }
        Ok(Response {
            status,
            headers,
            body: Vec::new(),
        })
    };
    let mut work = core::pin::pin!(work);
    let mut connection = core::pin::pin!(connection);
    let mut connection_done = false;
    poll_fn(|cx| {
        if let Poll::Ready(result) = work.as_mut().poll(cx) {
            return Poll::Ready(result);
        }
        let mut connection_failed = false;
        if !connection_done {
            match connection.as_mut().poll(cx) {
                Poll::Ready(Err(_)) => connection_failed = true,
                Poll::Ready(Ok(())) => connection_done = true,
                Poll::Pending => {}
            }
        }
        // Polling the connection can deliver END_STREAM and then observe the
        // peer closing its transport in the same poll. A complete framed
        // response takes precedence over that later connection error.
        match work.as_mut().poll(cx) {
            Poll::Ready(result) => Poll::Ready(result),
            Poll::Pending if connection_failed => Poll::Ready(Err(NetError::Network)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

async fn get_h1<S: Read + Write + Unpin>(
    mut stream: S,
    host: &str,
    path: &str,
    headers: &[(String, String)],
    maximum: usize,
    sink: &mut impl FnMut(u16, &[u8]) -> Result<(), NetError>,
) -> Result<Response, NetError> {
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (key, value) in headers {
        request.push_str(&format!("{key}: {value}\r\n"));
    }
    request.push_str("\r\n");
    let mut offset = 0;
    poll_fn(|cx| match stream.write(&request.as_bytes()[offset..]) {
        Ok(0) => Poll::Ready(Err(NetError::Network)),
        Ok(n) => {
            offset += n;
            if offset == request.len() {
                Poll::Ready(Ok(()))
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
        Err(_) => Poll::Ready(Err(NetError::Network)),
    })
    .await?;
    let mut decoder = crate::http_stream::Decoder::new(maximum);
    poll_fn(|cx| {
        let mut chunk = [0; 8192];
        match stream.read(&mut chunk) {
            Ok(0) => Poll::Ready(Ok(())),
            Ok(n) => {
                if let Err(error) = decoder.feed(&chunk[..n], sink) {
                    return Poll::Ready(Err(error));
                }
                if decoder.done() {
                    return Poll::Ready(Ok(()));
                }
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(_) => Poll::Ready(Err(NetError::Network)),
        }
    })
    .await?;
    decoder.finish()
}
