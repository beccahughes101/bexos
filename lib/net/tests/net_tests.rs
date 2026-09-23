use bexos_net::{doh, http1, nts};

#[test]
fn http1_get_request_encodes_host_and_path() {
    let mut out = Vec::new();
    http1::encode_get("example.com", "/index.html", &mut out).unwrap();
    assert_eq!(
        core::str::from_utf8(&out).unwrap(),
        "GET /index.html HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
    );
}

#[test]
fn http1_status_code_reads_success_line() {
    assert_eq!(
        http1::status_code(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap(),
        204
    );
}

#[test]
fn doh_post_wraps_dns_message() {
    let out = doh::encode_post("/dns-query", "dns.example", b"\x12\x34").unwrap();
    let text = core::str::from_utf8(&out).unwrap();
    assert!(text.starts_with("POST /dns-query HTTP/1.1\r\nHost: dns.example\r\n"));
    assert!(text.contains("Content-Type: application/dns-message\r\n"));
    assert!(out.ends_with(b"\x12\x34"));
}

#[test]
fn nts_cookie_validation_bounds_size() {
    assert!(nts::validate_cookie(&[0; 16]).is_ok());
    assert!(nts::validate_cookie(&[0; 15]).is_err());
}

#[test]
fn streaming_http_handles_every_fragment_boundary_and_rejects_truncation() {
    use bexos_net::http_stream::Decoder;
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n"
            .as_slice(),
    ] {
        for width in 1..response.len() {
            let mut decoder = Decoder::new(4);
            let mut bytes = Vec::new();
            for chunk in response.chunks(width) {
                decoder
                    .feed(chunk, &mut |status, chunk| {
                        assert_eq!(status, 200);
                        bytes.extend_from_slice(chunk);
                        Ok(())
                    })
                    .unwrap();
            }
            assert_eq!(decoder.finish().unwrap().status, 200);
            assert_eq!(bytes, b"abcd");
        }
    }
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabc".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n".as_slice(),
    ] {
        let mut decoder = Decoder::new(4);
        decoder.feed(response, &mut |_, _| Ok(())).unwrap();
        assert!(decoder.finish().is_err());
    }
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 4\r\n\r\n".as_slice(),
    ] {
        assert!(Decoder::new(4).feed(response, &mut |_, _| Ok(())).is_err());
    }
}
