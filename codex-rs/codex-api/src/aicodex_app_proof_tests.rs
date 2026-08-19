use super::HEADER_NAME;
use super::authorization_key_tag;
use super::hex_encode;
use super::path_from_url_or_path;
use super::sign;
use http::HeaderMap;
use http::HeaderValue;
use http::header::AUTHORIZATION;

#[test]
fn path_normalizes_full_url_and_bare_path() {
    assert_eq!(
        path_from_url_or_path("https://api.example.com/v1/responses?foo=1"),
        "/v1/responses"
    );
    assert_eq!(
        path_from_url_or_path("/v1/chat/completions"),
        "/v1/chat/completions"
    );
    assert_eq!(
        path_from_url_or_path("https://gw.example/v1/messages?beta=true"),
        "/v1/messages"
    );
    assert_eq!(path_from_url_or_path("messages"), "/messages");
    assert_eq!(path_from_url_or_path("responses"), "/responses");
    assert_eq!(
        path_from_url_or_path("https://gw.example/v1/messages/"),
        "/v1/messages"
    );
}

#[test]
fn key_tag_hashes_authorization() {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer sk-test"));
    let tag = authorization_key_tag(&headers);
    assert_eq!(tag.len(), 16);
    assert_eq!(tag, hex_encode(&sha256_8(b"Bearer sk-test")));
}

#[test]
fn key_tag_hashes_x_api_key_for_claude() {
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_static("sk-ant-test"));
    let tag = authorization_key_tag(&headers);
    assert_eq!(tag, hex_encode(&sha256_8(b"sk-ant-test")));
}

#[test]
fn sign_is_stable_for_same_inputs() {
    let first = sign(
        "secret",
        1_771_450_000,
        "nonce-1",
        "POST",
        "/v1/responses",
        "abcd",
    );
    let second = sign(
        "secret",
        1_771_450_000,
        "nonce-1",
        "post",
        "/v1/responses",
        "abcd",
    );
    assert_eq!(first, second);
    assert_eq!(
        first,
        "299ea3146200e9406c6f09650ebaaf9274b5dda0a4b6e9197bc6831018e59ab9"
    );
    assert_ne!(
        first,
        sign(
            "secret",
            1_771_450_000,
            "nonce-2",
            "POST",
            "/v1/responses",
            "abcd"
        )
    );
}

#[test]
fn apply_to_headers_writes_v1_proof() {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer sk-test"));
    super::apply_to_headers(&mut headers, "POST", "https://gw.example/v1/responses");
    let value = headers
        .get(HEADER_NAME)
        .and_then(|header| header.to_str().ok())
        .expect("proof header");
    assert!(value.starts_with("v1;ts="), "{value}");
    assert!(value.contains(";nonce="), "{value}");
    assert!(value.contains(";mac="), "{value}");
}

fn sha256_8(raw: &[u8]) -> [u8; 8] {
    use sha2::Digest;
    use sha2::Sha256;
    let digest = Sha256::digest(raw);
    digest[..8].try_into().expect("8 bytes")
}
