//! Reading JWT claims for display and the team check.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TokenClaims {
    pub sub: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub exp: i64,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    #[serde(default)]
    pub team_id: Option<String>,
}

fn default_token_type() -> String {
    "user".to_string()
}

impl TokenClaims {
    pub fn is_system_client(&self) -> bool {
        self.token_type == "system_client"
    }
}

/// Payload of a JWT, without checking the signature. Use it only for display
/// and to stop requests that the server would answer for another team.
pub fn decode_claims(token: &str) -> Option<TokenClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    fn jwt(claims: serde_json::Value) -> String {
        format!("{}.{}.sig", URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#), URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    #[test]
    fn decodes_system_client_claims() {
        let claims = decode_claims(&jwt(json!({
            "sub": "sc1", "username": "ci-bot", "is_admin": false, "exp": 4102444800i64,
            "token_type": "system_client", "team_id": "t1", "roles": [], "iat": 1
        })))
        .unwrap();
        assert!(claims.is_system_client());
        assert_eq!(claims.team_id.as_deref(), Some("t1"));
        assert_eq!(claims.exp, 4102444800);
    }

    #[test]
    fn user_tokens_default_to_user_type() {
        let claims = decode_claims(&jwt(json!({ "sub": "u1", "username": "alice", "exp": 1 }))).unwrap();
        assert!(!claims.is_system_client());
        assert_eq!(claims.token_type, "user");
    }

    #[test]
    fn opaque_tokens_have_no_claims() {
        assert_eq!(decode_claims("not-a-jwt"), None);
        assert_eq!(decode_claims("a.!!!.c"), None);
    }
}
