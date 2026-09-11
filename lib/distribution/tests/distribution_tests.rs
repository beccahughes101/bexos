use bexos_distribution::{
    DistributionClient, DistributionError, DistributionPolicy, DistributionTransport, HttpResponse,
    HttpsUrl, parse_http1_response, parse_https_url,
};

#[test]
fn strict_https_url_policy_rejects_unsafe_forms() {
    assert!(parse_https_url("https://example.com/app.bex").is_ok());
    assert_eq!(
        parse_https_url("http://example.com/app.bex"),
        Err(DistributionError::InvalidUrl)
    );
    assert_eq!(
        parse_https_url("https://user@example.com/app.bex"),
        Err(DistributionError::InvalidUrl)
    );
    assert_eq!(
        parse_https_url("https://example.com/app.bex#frag"),
        Err(DistributionError::InvalidUrl)
    );
}

#[test]
fn direct_installs_may_use_any_https_origin_when_policy_allows() {
    let policy = policy();
    let mut client = DistributionClient::new(policy, MockTransport::ok(b"archive".to_vec()));
    assert_eq!(
        client
            .fetch_direct_app("https://apps.example/pkg.bex")
            .unwrap(),
        b"archive"
    );
}

#[test]
fn tuf_fetches_are_origin_restricted_and_prod_template_fails_closed() {
    let mut client = DistributionClient::new(policy(), MockTransport::ok(b"meta".to_vec()));
    assert_eq!(
        client.fetch_tuf_metadata("https://evil.example/root.json"),
        Err(DistributionError::PolicyRejected)
    );

    let mut prod_template = policy();
    prod_template.deployable = false;
    prod_template.direct_web_installs = false;
    let mut client = DistributionClient::new(prod_template, MockTransport::ok(b"archive".to_vec()));
    assert_eq!(
        client.fetch_direct_app("https://apps.example/pkg.bex"),
        Err(DistributionError::PolicyRejected)
    );
}

#[test]
fn redirects_are_bounded_and_downgrade_is_rejected() {
    let mut client = DistributionClient::new(
        policy(),
        MockTransport::redirect("https://distribution.bexos.org/metadata/root.json"),
    );
    assert_eq!(
        client
            .fetch_tuf_metadata("https://distribution.bexos.org/metadata/timestamp.json")
            .unwrap(),
        b"ok"
    );

    let mut client = DistributionClient::new(
        policy(),
        MockTransport::redirect("http://distribution.bexos.org/root.json"),
    );
    assert_eq!(
        client.fetch_tuf_metadata("https://distribution.bexos.org/metadata/timestamp.json"),
        Err(DistributionError::Downgrade)
    );
}

#[test]
fn http1_parser_enforces_content_length_and_body_cap() {
    assert_eq!(
        parse_http1_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok", 8)
            .unwrap()
            .body,
        b"ok"
    );
    assert_eq!(
        parse_http1_response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nok", 8),
        Err(DistributionError::BadHttp)
    );
    assert_eq!(
        parse_http1_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok", 1),
        Err(DistributionError::BodyTooLarge)
    );
}

#[test]
fn http1_parser_accepts_chunked_and_rejects_ambiguous_framing() {
    assert_eq!(
        parse_http1_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhe\r\n3\r\nllo\r\n0\r\n\r\n",
            8,
        )
        .unwrap()
        .body,
        b"hello"
    );
    assert_eq!(
        parse_http1_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            8,
        ),
        Err(DistributionError::BadHttp)
    );
    assert_eq!(
        parse_http1_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\ntoo-large\r\n0\r\n\r\n",
            8,
        ),
        Err(DistributionError::BodyTooLarge)
    );
}

fn policy() -> DistributionPolicy {
    DistributionPolicy {
        deployable: true,
        direct_web_installs: true,
        allowed_tuf_origins: vec!["https://distribution.bexos.org".into()],
        max_metadata_bytes: 1024,
        max_target_bytes: 1024,
        max_redirects: 1,
    }
}

struct MockTransport {
    response: MockResponse,
}

enum MockResponse {
    Ok(Vec<u8>),
    Redirect(&'static str),
}

impl MockTransport {
    fn ok(body: Vec<u8>) -> Self {
        Self {
            response: MockResponse::Ok(body),
        }
    }

    fn redirect(location: &'static str) -> Self {
        Self {
            response: MockResponse::Redirect(location),
        }
    }
}

impl DistributionTransport for MockTransport {
    fn get(
        &mut self,
        _url: &HttpsUrl,
        _max_bytes: usize,
    ) -> Result<HttpResponse, DistributionError> {
        match core::mem::replace(&mut self.response, MockResponse::Ok(b"ok".to_vec())) {
            MockResponse::Ok(body) => Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body,
            }),
            MockResponse::Redirect(location) => Ok(HttpResponse {
                status: 302,
                headers: vec![("location".into(), location.into())],
                body: Vec::new(),
            }),
        }
    }
}
