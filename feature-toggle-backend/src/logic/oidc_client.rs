//! Minimal OpenID Connect relying-party client: discovery, JWKS, authorization code
//! exchange with PKCE, id_token validation and userinfo.
//!
//! Built on `jsonwebtoken` (already used for FluxGate's own tokens) and `reqwest`
//! rather than the `openidconnect` crate, so every IdP response goes through the
//! 1 MiB-capped fetch helper in [`crate::logic::oidc_http`], the discovery + JWKS
//! cache is under our control, and the accepted signature algorithms are an explicit
//! allow list (RS256, ES256, PS256; never `none` or HS*).
//!
//! Discovery documents and JWKS are cached per issuer URL for ten minutes. A token
//! signed with an unknown `kid` triggers one JWKS refetch.

use crate::logic::oidc_http::{FetchError, get_json, idp_http_client, read_json_capped};
use crate::logic::sso_provider::{discovery_url, same_issuer};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

/// How long a discovery document and its JWKS are reused.
pub const METADATA_CACHE_TTL: Duration = Duration::from_secs(600);

/// Clock skew tolerated for `exp`, `nbf` and `iat`.
pub const CLOCK_LEEWAY_SECONDS: u64 = 60;

/// Signature algorithms FluxGate accepts for id_tokens.
const SUPPORTED_ALGORITHMS: [Algorithm; 3] = [Algorithm::RS256, Algorithm::ES256, Algorithm::PS256];

/// Errors of the OIDC client. Messages never contain tokens, codes, secrets or
/// response bodies, so they can be logged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OidcError {
    /// Discovery or JWKS could not be loaded or is unusable.
    #[error("discovery failed: {0}")]
    Discovery(String),
    /// The token endpoint rejected the code or returned an unusable response.
    #[error("token exchange failed: {0}")]
    TokenExchange(String),
    /// The id_token failed validation.
    #[error("id_token invalid: {0}")]
    InvalidToken(String),
    /// The userinfo endpoint failed.
    #[error("userinfo failed: {0}")]
    Userinfo(String),
}

/// The subset of the OpenID Provider metadata FluxGate uses.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
    pub jwks_uri: String,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
    #[serde(default)]
    pub id_token_signing_alg_values_supported: Vec<String>,
}

impl ProviderMetadata {
    /// The algorithms accepted for this provider: the supported ones the provider
    /// advertises, or RS256 (the OIDC default) when it advertises none.
    pub fn accepted_algorithms(&self) -> Vec<Algorithm> {
        if self.id_token_signing_alg_values_supported.is_empty() {
            return vec![Algorithm::RS256];
        }
        SUPPORTED_ALGORITHMS
            .into_iter()
            .filter(|alg| {
                self.id_token_signing_alg_values_supported
                    .iter()
                    .any(|value| value == algorithm_name(*alg))
            })
            .collect()
    }

    /// `client_secret_basic` unless the provider advertises only
    /// `client_secret_post` (an empty list means the default, basic).
    pub fn client_auth_method(&self) -> ClientAuthMethod {
        let methods = &self.token_endpoint_auth_methods_supported;
        let supports = |name: &str| methods.iter().any(|m| m == name);
        if !methods.is_empty() && !supports("client_secret_basic") && supports("client_secret_post")
        {
            ClientAuthMethod::Post
        } else {
            ClientAuthMethod::Basic
        }
    }
}

fn algorithm_name(alg: Algorithm) -> &'static str {
    match alg {
        Algorithm::RS256 => "RS256",
        Algorithm::ES256 => "ES256",
        Algorithm::PS256 => "PS256",
        _ => "",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuthMethod {
    Basic,
    Post,
}

/// A signing key from the JWKS with the raw `alg` / `use` members kept for checks
/// the typed `Jwk` does not expose uniformly.
#[derive(Clone)]
struct SigningJwk {
    kid: Option<String>,
    alg: Option<String>,
    jwk: Jwk,
    kty: String,
    crv: Option<String>,
}

impl SigningJwk {
    /// Whether this key can verify a signature made with `alg`.
    fn usable_for(&self, alg: Algorithm) -> bool {
        if let Some(key_alg) = self.alg.as_deref()
            && key_alg != algorithm_name(alg)
        {
            return false;
        }
        match alg {
            Algorithm::RS256 | Algorithm::PS256 => self.kty == "RSA",
            Algorithm::ES256 => self.kty == "EC" && self.crv.as_deref() == Some("P-256"),
            _ => false,
        }
    }
}

/// Parses a JWKS, keeping only RSA and P-256 EC signing keys. Keys that do not
/// parse (unknown algorithms, encryption keys, symmetric keys) are skipped.
fn parse_jwks(value: &serde_json::Value) -> Vec<SigningJwk> {
    let Some(keys) = value.get("keys").and_then(|k| k.as_array()) else {
        return Vec::new();
    };
    keys.iter()
        .filter_map(|raw| {
            let kty = raw.get("kty")?.as_str()?.to_string();
            if kty != "RSA" && kty != "EC" {
                return None;
            }
            if raw
                .get("use")
                .and_then(|u| u.as_str())
                .is_some_and(|u| u != "sig")
            {
                return None;
            }
            let jwk: Jwk = serde_json::from_value(raw.clone()).ok()?;
            Some(SigningJwk {
                kid: raw.get("kid").and_then(|k| k.as_str()).map(str::to_string),
                alg: raw.get("alg").and_then(|a| a.as_str()).map(str::to_string),
                crv: raw.get("crv").and_then(|c| c.as_str()).map(str::to_string),
                kty,
                jwk,
            })
        })
        .collect()
}

struct CachedProvider {
    metadata: Arc<ProviderMetadata>,
    keys: Arc<Vec<SigningJwk>>,
    fetched_at: Instant,
}

/// Token endpoint response. `Debug` redacts the tokens.
#[derive(Clone, Deserialize)]
pub struct TokenResponse {
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_type", &self.token_type)
            .finish()
    }
}

/// Input of the authorization code exchange. `Debug` redacts the code, verifier
/// and client secret.
pub struct CodeExchange<'a> {
    pub client_id: &'a str,
    pub client_secret: Option<&'a str>,
    pub code: &'a str,
    pub pkce_verifier: &'a str,
    pub redirect_uri: &'a str,
}

impl std::fmt::Debug for CodeExchange<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeExchange")
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.map(|_| "<redacted>"))
            .field("code", &"<redacted>")
            .field("pkce_verifier", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

/// Validated id_token claims FluxGate reads. `raw` holds every claim (for the
/// groups claim used by role sync).
#[derive(Debug, Clone)]
pub struct IdTokenClaims {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub preferred_username: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub name: Option<String>,
    pub raw: serde_json::Value,
}

fn string_claim(claims: &serde_json::Value, name: &str) -> Option<String> {
    claims
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// `email_verified` is a boolean per spec; some IdPs send the string `"true"`.
pub fn claim_is_true(claims: &serde_json::Value, name: &str) -> bool {
    match claims.get(name) {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

impl IdTokenClaims {
    pub fn from_value(raw: serde_json::Value) -> Result<Self, OidcError> {
        let subject = string_claim(&raw, "sub")
            .ok_or_else(|| OidcError::InvalidToken("missing sub".to_string()))?;
        Ok(Self {
            subject,
            email: string_claim(&raw, "email"),
            email_verified: claim_is_true(&raw, "email_verified"),
            preferred_username: string_claim(&raw, "preferred_username"),
            given_name: string_claim(&raw, "given_name"),
            family_name: string_claim(&raw, "family_name"),
            name: string_claim(&raw, "name"),
            raw,
        })
    }

    /// Fills a missing email (and its verified flag) from userinfo claims. The
    /// userinfo `sub` must equal the id_token `sub` (OIDC Core 5.3.2).
    pub fn merge_userinfo(&mut self, userinfo: &serde_json::Value) -> Result<(), OidcError> {
        let sub = string_claim(userinfo, "sub");
        if sub.as_deref() != Some(self.subject.as_str()) {
            return Err(OidcError::Userinfo(
                "userinfo sub does not match id_token sub".to_string(),
            ));
        }
        if self.email.is_none() {
            self.email = string_claim(userinfo, "email");
            self.email_verified = claim_is_true(userinfo, "email_verified");
        }
        if self.preferred_username.is_none() {
            self.preferred_username = string_claim(userinfo, "preferred_username");
        }
        if self.given_name.is_none() {
            self.given_name = string_claim(userinfo, "given_name");
        }
        if self.family_name.is_none() {
            self.family_name = string_claim(userinfo, "family_name");
        }
        if self.name.is_none() {
            self.name = string_claim(userinfo, "name");
        }
        Ok(())
    }
}

/// 32 random bytes, base64url without padding (state, nonce, PKCE verifier,
/// one-time codes).
pub fn random_token() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE S256 challenge of `verifier`.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `application/x-www-form-urlencoded` encoding of one value (RFC 6749 2.3.1
/// requires it for the client id and secret in HTTP Basic authentication).
fn form_urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// OIDC client with a per-issuer discovery + JWKS cache. Cheap to clone.
#[derive(Clone)]
pub struct OidcClient {
    http: reqwest::Client,
    cache: Arc<Mutex<HashMap<String, Arc<CachedProvider>>>>,
    ttl: Duration,
}

impl OidcClient {
    pub fn new() -> Result<Self, OidcError> {
        Self::with_ttl(METADATA_CACHE_TTL)
    }

    pub fn with_ttl(ttl: Duration) -> Result<Self, OidcError> {
        let http = idp_http_client()
            .map_err(|_| OidcError::Discovery("could not create HTTP client".to_string()))?;
        Ok(Self {
            http,
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        })
    }

    fn cached(&self, issuer_url: &str) -> Option<Arc<CachedProvider>> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache
            .get(issuer_url)
            .filter(|entry| entry.fetched_at.elapsed() < self.ttl)
            .cloned()
    }

    fn store(&self, issuer_url: &str, entry: Arc<CachedProvider>) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(issuer_url.to_string(), entry);
    }

    /// Discovery metadata of `issuer_url` (cached).
    pub async fn metadata(&self, issuer_url: &str) -> Result<Arc<ProviderMetadata>, OidcError> {
        Ok(self.provider(issuer_url).await?.metadata.clone())
    }

    async fn provider(&self, issuer_url: &str) -> Result<Arc<CachedProvider>, OidcError> {
        if let Some(entry) = self.cached(issuer_url) {
            return Ok(entry);
        }
        let metadata: ProviderMetadata = get_json(&self.http, &discovery_url(issuer_url))
            .await
            .map_err(|err| OidcError::Discovery(format!("discovery document: {err}")))?;
        if !same_issuer(&metadata.issuer, issuer_url) {
            return Err(OidcError::Discovery(
                "discovery issuer does not match the configured issuer".to_string(),
            ));
        }
        let keys = self.fetch_keys(&metadata.jwks_uri).await?;
        let entry = Arc::new(CachedProvider {
            metadata: Arc::new(metadata),
            keys: Arc::new(keys),
            fetched_at: Instant::now(),
        });
        self.store(issuer_url, entry.clone());
        Ok(entry)
    }

    async fn fetch_keys(&self, jwks_uri: &str) -> Result<Vec<SigningJwk>, OidcError> {
        let jwks: serde_json::Value = get_json(&self.http, jwks_uri)
            .await
            .map_err(|err| OidcError::Discovery(format!("JWKS: {err}")))?;
        Ok(parse_jwks(&jwks))
    }

    /// Refetches the JWKS of a cached provider (unknown `kid`) and stores it.
    async fn refresh_keys(
        &self,
        issuer_url: &str,
        entry: &CachedProvider,
    ) -> Result<Arc<Vec<SigningJwk>>, OidcError> {
        let keys = Arc::new(self.fetch_keys(&entry.metadata.jwks_uri).await?);
        self.store(
            issuer_url,
            Arc::new(CachedProvider {
                metadata: entry.metadata.clone(),
                keys: keys.clone(),
                fetched_at: Instant::now(),
            }),
        );
        Ok(keys)
    }

    /// Exchanges an authorization code (with its PKCE verifier) at the token endpoint.
    pub async fn exchange_code(
        &self,
        metadata: &ProviderMetadata,
        input: CodeExchange<'_>,
    ) -> Result<TokenResponse, OidcError> {
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", input.code),
            ("redirect_uri", input.redirect_uri),
            ("code_verifier", input.pkce_verifier),
        ];
        let mut request = self.http.post(&metadata.token_endpoint);
        match (input.client_secret, metadata.client_auth_method()) {
            (Some(secret), ClientAuthMethod::Basic) => {
                let credentials = format!(
                    "{}:{}",
                    form_urlencode(input.client_id),
                    form_urlencode(secret)
                );
                request = request.header(
                    reqwest::header::AUTHORIZATION,
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(credentials)
                    ),
                );
            }
            (Some(secret), ClientAuthMethod::Post) => {
                form.push(("client_id", input.client_id));
                form.push(("client_secret", secret));
            }
            (None, _) => form.push(("client_id", input.client_id)),
        }
        let response = request
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&form)
            .send()
            .await
            .map_err(|err| OidcError::TokenExchange(FetchError::from(err).to_string()))?;
        let tokens: TokenResponse = read_json_capped(response)
            .await
            .map_err(|err| OidcError::TokenExchange(err.to_string()))?;
        if tokens.id_token.is_none() {
            return Err(OidcError::TokenExchange(
                "token response has no id_token".to_string(),
            ));
        }
        Ok(tokens)
    }

    /// Validates an id_token: signature against the provider JWKS with an allowed
    /// algorithm, `iss` equal to the discovery issuer, `aud` containing the client
    /// id, `exp` / `nbf` / `iat` with 60 seconds leeway, and `nonce`.
    pub async fn validate_id_token(
        &self,
        issuer_url: &str,
        id_token: &str,
        client_id: &str,
        expected_nonce: &str,
    ) -> Result<IdTokenClaims, OidcError> {
        let entry = self.provider(issuer_url).await?;
        let header = decode_header(id_token)
            .map_err(|_| OidcError::InvalidToken("malformed token header".to_string()))?;
        let alg = header.alg;
        if !entry.metadata.accepted_algorithms().contains(&alg) {
            return Err(OidcError::InvalidToken(format!(
                "signature algorithm {alg:?} is not accepted"
            )));
        }

        let mut keys = entry.keys.clone();
        if header.kid.is_some() && !has_key(&keys, header.kid.as_deref(), alg) {
            keys = self.refresh_keys(issuer_url, &entry).await?;
        }
        let candidates: Vec<&SigningJwk> = keys
            .iter()
            .filter(|key| key.usable_for(alg))
            .filter(|key| header.kid.is_none() || key.kid == header.kid)
            .collect();
        if candidates.is_empty() {
            return Err(OidcError::InvalidToken(
                "no matching signing key".to_string(),
            ));
        }

        let mut validation = Validation::new(alg);
        validation.algorithms = vec![alg];
        validation.leeway = CLOCK_LEEWAY_SECONDS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.validate_aud = true;
        validation.set_issuer(&[entry.metadata.issuer.as_str()]);
        validation.set_audience(&[client_id]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        let mut last_error = OidcError::InvalidToken("signature verification failed".to_string());
        let mut verified = None;
        for key in candidates {
            let decoding_key = match DecodingKey::from_jwk(&key.jwk) {
                Ok(k) => k,
                Err(_) => continue,
            };
            match decode::<serde_json::Value>(id_token, &decoding_key, &validation) {
                Ok(data) => {
                    verified = Some(data.claims);
                    break;
                }
                Err(err) => last_error = OidcError::InvalidToken(describe_jwt_error(&err)),
            }
        }
        let claims = verified.ok_or(last_error)?;

        let now = chrono::Utc::now().timestamp();
        match claims.get("iat").and_then(|v| v.as_i64()) {
            Some(iat) if iat <= now + CLOCK_LEEWAY_SECONDS as i64 => {}
            Some(_) => return Err(OidcError::InvalidToken("iat is in the future".to_string())),
            None => return Err(OidcError::InvalidToken("missing iat".to_string())),
        }
        let nonce = claims.get("nonce").and_then(|v| v.as_str()).unwrap_or("");
        if !bool::from(nonce.as_bytes().ct_eq(expected_nonce.as_bytes())) {
            return Err(OidcError::InvalidToken("nonce mismatch".to_string()));
        }
        // With several audiences, the authorized party must be this client.
        if let Some(azp) = claims.get("azp").and_then(|v| v.as_str())
            && azp != client_id
        {
            return Err(OidcError::InvalidToken("azp mismatch".to_string()));
        }

        IdTokenClaims::from_value(claims)
    }

    /// Calls the userinfo endpoint with the access token.
    pub async fn userinfo(
        &self,
        metadata: &ProviderMetadata,
        access_token: &str,
    ) -> Result<serde_json::Value, OidcError> {
        let endpoint = metadata
            .userinfo_endpoint
            .as_deref()
            .ok_or_else(|| OidcError::Userinfo("provider has no userinfo endpoint".to_string()))?;
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|err| OidcError::Userinfo(FetchError::from(err).to_string()))?;
        read_json_capped(response)
            .await
            .map_err(|err| OidcError::Userinfo(err.to_string()))
    }
}

fn has_key(keys: &[SigningJwk], kid: Option<&str>, alg: Algorithm) -> bool {
    keys.iter()
        .any(|key| key.kid.as_deref() == kid && key.usable_for(alg))
}

fn describe_jwt_error(err: &jsonwebtoken::errors::Error) -> String {
    use jsonwebtoken::errors::ErrorKind;
    match err.kind() {
        ErrorKind::InvalidSignature => "invalid signature".to_string(),
        ErrorKind::ExpiredSignature => "token expired".to_string(),
        ErrorKind::ImmatureSignature => "token not yet valid".to_string(),
        ErrorKind::InvalidIssuer => "issuer mismatch".to_string(),
        ErrorKind::InvalidAudience => "audience mismatch".to_string(),
        ErrorKind::MissingRequiredClaim(claim) => format!("missing {claim}"),
        ErrorKind::InvalidAlgorithm => "algorithm does not match key".to_string(),
        _ => "token could not be verified".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(algs: &[&str], auth: &[&str]) -> ProviderMetadata {
        ProviderMetadata {
            issuer: "https://idp.example".into(),
            authorization_endpoint: "https://idp.example/auth".into(),
            token_endpoint: "https://idp.example/token".into(),
            userinfo_endpoint: None,
            jwks_uri: "https://idp.example/jwks".into(),
            token_endpoint_auth_methods_supported: auth.iter().map(|s| s.to_string()).collect(),
            id_token_signing_alg_values_supported: algs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn accepted_algorithms_never_include_none_or_hmac() {
        let m = metadata(&["none", "HS256", "RS256", "ES256", "PS256", "RS512"], &[]);
        assert_eq!(
            m.accepted_algorithms(),
            vec![Algorithm::RS256, Algorithm::ES256, Algorithm::PS256]
        );
        assert_eq!(
            metadata(&[], &[]).accepted_algorithms(),
            vec![Algorithm::RS256]
        );
        assert!(
            metadata(&["HS256", "none"], &[])
                .accepted_algorithms()
                .is_empty()
        );
    }

    #[test]
    fn client_auth_prefers_basic_and_falls_back_to_post() {
        assert_eq!(
            metadata(&[], &[]).client_auth_method(),
            ClientAuthMethod::Basic
        );
        assert_eq!(
            metadata(&[], &["client_secret_post", "client_secret_basic"]).client_auth_method(),
            ClientAuthMethod::Basic
        );
        assert_eq!(
            metadata(&[], &["client_secret_post"]).client_auth_method(),
            ClientAuthMethod::Post
        );
        assert_eq!(
            metadata(&[], &["private_key_jwt"]).client_auth_method(),
            ClientAuthMethod::Basic
        );
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        // RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_tokens_are_32_bytes_base64url() {
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
        assert_eq!(URL_SAFE_NO_PAD.decode(&a).unwrap().len(), 32);
        assert!(!a.contains('=') && !a.contains('+') && !a.contains('/'));
    }

    #[test]
    fn form_urlencode_escapes_reserved_characters() {
        assert_eq!(form_urlencode("a b:c%d"), "a+b%3Ac%25d");
        assert_eq!(form_urlencode("Abc-._*9"), "Abc-._*9");
    }

    #[test]
    fn jwks_parsing_skips_symmetric_encryption_and_unknown_keys() {
        let jwks = serde_json::json!({"keys": [
            {"kty": "oct", "k": "c2VjcmV0", "kid": "hmac"},
            {"kty": "RSA", "use": "enc", "n": "AQAB", "e": "AQAB", "kid": "enc"},
            {"kty": "OKP", "crv": "Ed25519", "x": "AAAA", "kid": "ed"},
            {"kty": "RSA", "use": "sig", "alg": "RS256", "n": "AQAB", "e": "AQAB", "kid": "sig"}
        ]});
        let keys = parse_jwks(&jwks);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].kid.as_deref(), Some("sig"));
        assert!(keys[0].usable_for(Algorithm::RS256));
        assert!(!keys[0].usable_for(Algorithm::PS256), "alg pinned to RS256");
        assert!(!keys[0].usable_for(Algorithm::ES256));
        assert!(!keys[0].usable_for(Algorithm::HS256));
    }

    #[test]
    fn email_verified_accepts_bool_and_string_true() {
        let v = serde_json::json!({"a": true, "b": "true", "c": "false", "d": 1});
        assert!(claim_is_true(&v, "a"));
        assert!(claim_is_true(&v, "b"));
        assert!(!claim_is_true(&v, "c"));
        assert!(!claim_is_true(&v, "d"));
        assert!(!claim_is_true(&v, "missing"));
    }

    #[test]
    fn userinfo_merge_requires_matching_sub() {
        let mut claims = IdTokenClaims::from_value(serde_json::json!({"sub": "u1"})).unwrap();
        assert!(
            claims
                .merge_userinfo(&serde_json::json!({"sub": "u2", "email": "x@y"}))
                .is_err()
        );
        assert!(claims.email.is_none());
        claims
            .merge_userinfo(
                &serde_json::json!({"sub": "u1", "email": "x@y", "email_verified": true}),
            )
            .unwrap();
        assert_eq!(claims.email.as_deref(), Some("x@y"));
        assert!(claims.email_verified);
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let exchange = CodeExchange {
            client_id: "client",
            client_secret: Some("s3cret-value"),
            code: "auth-code-value",
            pkce_verifier: "verifier-value",
            redirect_uri: "https://app/cb",
        };
        let text = format!("{exchange:?}");
        for secret in ["s3cret-value", "auth-code-value", "verifier-value"] {
            assert!(!text.contains(secret), "{text}");
        }
        let tokens = TokenResponse {
            id_token: Some("id-token-value".into()),
            access_token: Some("access-token-value".into()),
            token_type: Some("Bearer".into()),
        };
        let text = format!("{tokens:?}");
        assert!(!text.contains("id-token-value") && !text.contains("access-token-value"));
    }
}
