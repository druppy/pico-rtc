use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::types::TurnCredentials;

// TURN REST API (RFC 5389 + de-facto standard used by coturn) mandates HMAC-SHA1.
// This is NOT a weakness: HMAC security does not depend on the underlying hash's
// collision resistance. HMAC-SHA1 remains secure against all known attacks.
// Changing to SHA256 here would break coturn compatibility.
type HmacSha1 = Hmac<Sha1>;

/// Generate ephemeral TURN credentials (TURN REST API spec).
///
/// username = "<expires_unix>:<random_id>"
/// credential = base64(HMAC-SHA1(secret, username))
///
/// coturn must be configured with `use-auth-secret` and the same `static-auth-secret`.
pub fn generate_credentials(secret: &str, host: &str, port: u16, ttl_secs: u64) -> TurnCredentials {
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + ttl_secs;

    let random_id = format!("{:016x}", rand::random::<u64>());
    let username = format!("{expires}:{random_id}");

    let mut mac =
        HmacSha1::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    TurnCredentials {
        urls: vec![
            format!("turn:{host}:{port}"),
            format!("turn:{host}:{port}?transport=udp"),
            format!("stun:{host}:{port}"),
        ],
        username,
        credential,
        ttl_secs,
    }
}

/// Hash a user-supplied identifier with SHA-256 (for cookies, internal keys, etc.)
pub fn hash_id(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}
