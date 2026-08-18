use codex_client::Request;
use hmac::Hmac;
use hmac::Mac;
use http::header::AUTHORIZATION;
use http::HeaderMap;
use http::HeaderValue;
use sha2::Sha256;
use url::Url;
use uuid::Uuid;

pub(crate) const HEADER_NAME: &str = "x-aicodex-app-proof";
const DEFAULT_APP_PROOF_SECRET: &str =
    "2281bd21edfe32b9690af10d417e75805a8337b3f73db59a5b717e8237c79098";

const HEADER_API_KEY: &str = "x-api-key";

type HmacSha256 = Hmac<Sha256>;

pub(crate) fn apply_to_request(request: &mut Request) {
    apply_to_headers(&mut request.headers, request.method.as_str(), &request.url);
}

pub(crate) fn apply_to_headers(headers: &mut HeaderMap, method: &str, url_or_path: &str) {
    headers.insert(
        HEADER_NAME,
        build_header_value(headers, method, url_or_path),
    );
}

fn build_header_value(headers: &HeaderMap, method: &str, url_or_path: &str) -> HeaderValue {
    let ts = now_unix();
    let nonce = Uuid::new_v4().as_simple().to_string();
    let path = path_from_url_or_path(url_or_path);
    let key_tag = authorization_key_tag(headers);
    let mac = sign(
        DEFAULT_APP_PROOF_SECRET,
        ts,
        &nonce,
        method,
        &path,
        &key_tag,
    );
    header_value(&format!("v1;ts={ts};nonce={nonce};mac={mac}"))
}

fn sign(secret: &str, ts: i64, nonce: &str, method: &str, path: &str, key_tag: &str) -> String {
    let canonical = format!(
        "{ts}\n{nonce}\n{}\n{path}\n{key_tag}",
        method.trim().to_ascii_uppercase()
    );
    hex_encode(&hmac_sha256(secret.as_bytes(), canonical.as_bytes()))
}

fn hmac_sha256(secret: &[u8], canonical: &[u8]) -> hmac::digest::Output<HmacSha256> {
    match HmacSha256::new_from_slice(secret) {
        Ok(mut signer) => {
            signer.update(canonical);
            signer.finalize().into_bytes()
        }
        // HMAC-SHA256 accepts arbitrary key lengths.
        Err(_) => unreachable!("HMAC-SHA256 accepts the official app proof secret"),
    }
}

fn header_value(value: &str) -> HeaderValue {
    match HeaderValue::from_str(value) {
        Ok(value) => value,
        Err(_) => unreachable!("official app proof header is ASCII"),
    }
}

fn authorization_key_tag(headers: &HeaderMap) -> String {
    let raw = headers
        .get(AUTHORIZATION)
        .or_else(|| headers.get(HEADER_API_KEY))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    if raw.is_empty() {
        return String::new();
    }
    use sha2::Digest;
    let digest = Sha256::digest(raw.as_bytes());
    hex_encode(&digest[..8])
}

fn path_from_url_or_path(value: &str) -> String {
    if let Ok(url) = Url::parse(value) {
        return normalize_app_proof_path(url.path());
    }
    normalize_app_proof_path(value)
}

fn normalize_app_proof_path(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "/".to_string();
    }
    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    if with_slash == "/" {
        return with_slash;
    }
    let stripped = with_slash.trim_end_matches('/');
    if stripped.is_empty() {
        "/".to_string()
    } else {
        stripped.to_string()
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
#[path = "aicodex_app_proof_tests.rs"]
mod tests;
