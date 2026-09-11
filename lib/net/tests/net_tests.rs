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
