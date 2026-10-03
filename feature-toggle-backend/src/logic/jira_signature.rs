//! HMAC check for native Jira webhooks (JI-45, design §3.5).
//!
//! Jira signs the raw request body with the webhook's secret and sends
//! `X-Hub-Signature: sha256=<hex>` (WebSub format).

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Header carrying the signature.
pub const SIGNATURE_HEADER: &str = "X-Hub-Signature";

const PREFIX: &str = "sha256=";
const HEX_LEN: usize = 64;

/// True when `header` is `sha256=<hex>` and equals HMAC-SHA256(secret, body). Constant time.
pub fn signature_matches(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(hex) = header.trim().strip_prefix(PREFIX) else {
        return false;
    };
    let Some(expected) = decode_hex(hex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    // `verify_slice` compares in constant time.
    mac.verify_slice(&expected).is_ok()
}

/// Exactly 64 hex digits (either case) to 32 bytes.
fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() != HEX_LEN || !hex.is_ascii() {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some((high * 16 + low) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::jira_integration::generate_secret;

    fn hex_lower(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex_lower(&mac.finalize().into_bytes()))
    }

    #[test]
    fn valid_signature_matches() {
        let s = generate_secret();
        assert!(signature_matches(&s, b"{}", &sign(&s, b"{}")));
    }

    #[test]
    fn upper_case_hex_matches() {
        let s = generate_secret();
        let sig = sign(&s, b"{}");
        let upper = format!("sha256={}", sig["sha256=".len()..].to_uppercase());
        assert!(signature_matches(&s, b"{}", &upper));
    }

    #[test]
    fn changed_body_fails() {
        let s = generate_secret();
        assert!(!signature_matches(&s, b"{ }", &sign(&s, b"{}")));
    }

    #[test]
    fn wrong_secret_fails() {
        let (a, b) = (generate_secret(), generate_secret());
        assert!(!signature_matches(&b, b"{}", &sign(&a, b"{}")));
    }

    #[test]
    fn sha1_prefix_fails() {
        let s = generate_secret();
        let sig = sign(&s, b"{}");
        let sha1 = sig.replacen("sha256=", "sha1=", 1);
        assert!(!signature_matches(&s, b"{}", &sha1));
    }

    #[test]
    fn missing_prefix_or_bad_length_fails() {
        let s = generate_secret();
        let hex = sign(&s, b"{}")["sha256=".len()..].to_string();
        assert!(!signature_matches(&s, b"{}", "abc"));
        assert!(!signature_matches(&s, b"{}", "sha256="));
        assert!(!signature_matches(&s, b"{}", &hex)); // no prefix
        assert!(!signature_matches(
            &s,
            b"{}",
            &format!("sha256={}", "zz".repeat(32))
        ));
        assert!(!signature_matches(
            &s,
            b"{}",
            &format!("sha256={}", &hex[..63])
        ));
        assert!(!signature_matches(&s, b"{}", &format!("sha256={hex}00")));
    }
}
