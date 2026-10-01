//! Admin logic for OIDC identity providers: validation, client secret handling and
//! provider discovery checks.
//!
//! Client secrets are write-only. A secret supplied through the environment variable
//! `FLUXGATE_SSO_<SLUG>_CLIENT_SECRET` wins over the stored one; otherwise the secret
//! is stored encrypted (see `logic::secret_box`) with the provider id as associated
//! data, so a stored ciphertext cannot be moved to another provider row.

use crate::Error;
use crate::database::entity::SsoProvider;
use crate::logic::oidc_http::{FetchError, get_json};
use crate::logic::secret_box::{SecretBox, SecretBoxError};
use serde::Deserialize;
use std::time::Duration;
use uuid::Uuid;

pub const ROLE_SYNC_AUTHORITATIVE: &str = "authoritative";
pub const ROLE_SYNC_ADDITIVE: &str = "additive";
pub const ROLE_SYNC_OFF: &str = "off";
const ROLE_SYNC_MODES: [&str; 3] = [ROLE_SYNC_AUTHORITATIVE, ROLE_SYNC_ADDITIVE, ROLE_SYNC_OFF];

pub const DEFAULT_SCOPES: [&str; 3] = ["openid", "email", "profile"];
pub const DEFAULT_GROUPS_CLAIM: &str = "groups";

const MAX_TEXT_LEN: usize = 255;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors of the SSO admin operations.
#[derive(Debug, thiserror::Error)]
pub enum SsoAdminError {
    /// 400 `invalid_input`.
    #[error("{0}")]
    Invalid(String),
    /// 400 `encryption_key_missing`: a client secret was supplied but no valid
    /// `FLUXGATE_ENCRYPTION_KEY` is configured.
    #[error("FLUXGATE_ENCRYPTION_KEY is not configured")]
    EncryptionKeyMissing,
    #[error(transparent)]
    Other(#[from] Error),
}

impl SsoAdminError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

/// Seals and opens stored client secrets. Cheap to clone; holds the key, or none when
/// `FLUXGATE_ENCRYPTION_KEY` is unset.
#[derive(Clone, Debug)]
pub struct SsoSecrets {
    secret_box: Option<std::sync::Arc<SecretBox>>,
}

impl SsoSecrets {
    /// Reads `FLUXGATE_ENCRYPTION_KEY`. An unset or invalid key leaves secrets disabled:
    /// saving a secret then fails with `encryption_key_missing`.
    pub fn from_env() -> Self {
        let value = std::env::var(crate::logic::secret_box::ENCRYPTION_KEY_ENV).ok();
        match SecretBox::from_key_value(value.as_deref()) {
            Ok(secret_box) => Self::with_box(secret_box),
            Err(SecretBoxError::KeyMissing) => Self::disabled(),
            Err(err) => {
                log::warn!("SSO client secret storage disabled: {err}");
                Self::disabled()
            }
        }
    }

    pub fn with_box(secret_box: SecretBox) -> Self {
        Self {
            secret_box: Some(std::sync::Arc::new(secret_box)),
        }
    }

    pub fn disabled() -> Self {
        Self { secret_box: None }
    }

    /// Encrypts `secret` for the provider; the provider id is the associated data.
    pub fn seal(&self, provider_id: Uuid, secret: &str) -> Result<String, SsoAdminError> {
        let secret_box = self
            .secret_box
            .as_ref()
            .ok_or(SsoAdminError::EncryptionKeyMissing)?;
        secret_box
            .encrypt_with_aad(secret, provider_id.as_bytes())
            .map_err(|err| match err {
                SecretBoxError::KeyMissing | SecretBoxError::KeyInvalid => {
                    SsoAdminError::EncryptionKeyMissing
                }
                _ => SsoAdminError::Other(Error::InvalidInput(
                    "Failed to encrypt client secret".to_string(),
                )),
            })
    }

    /// Decrypts a stored client secret of the provider.
    pub fn open(&self, provider_id: Uuid, sealed: &str) -> Result<String, SecretBoxError> {
        self.secret_box
            .as_ref()
            .ok_or(SecretBoxError::KeyMissing)?
            .decrypt_with_aad(sealed, provider_id.as_bytes())
    }
}

/// Environment variable that overrides the client secret of the provider with `slug`:
/// `FLUXGATE_SSO_<SLUG_UPPERCASE_WITH_UNDERSCORES>_CLIENT_SECRET`.
pub fn client_secret_env_var(slug: &str) -> String {
    let normalized: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("FLUXGATE_SSO_{normalized}_CLIENT_SECRET")
}

/// The client secret from the environment override, if set and not empty.
pub fn client_secret_from_env(slug: &str) -> Option<String> {
    std::env::var(client_secret_env_var(slug))
        .ok()
        .filter(|value| !value.is_empty())
}

/// The client secret to use with the IdP: the environment override first, then the
/// stored encrypted one. `Ok(None)` when no secret is configured (public client).
pub fn resolve_client_secret(
    provider: &SsoProvider,
    secrets: &SsoSecrets,
) -> Result<Option<String>, SecretBoxError> {
    if let Some(value) = client_secret_from_env(&provider.slug) {
        return Ok(Some(value));
    }
    match provider.client_secret_enc.as_deref() {
        Some(sealed) => secrets.open(provider.id, sealed).map(Some),
        None => Ok(None),
    }
}

/// Validated fields for creating a provider (defaults applied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedProvider {
    pub slug: String,
    pub display_name: String,
    pub issuer_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub allowed_email_domains: Vec<String>,
    pub jit_provisioning: bool,
    pub allow_email_linking: bool,
    pub role_sync_mode: String,
    pub enabled: bool,
}

/// Provider fields as received from the API; every field optional so one shape serves
/// create (required fields enforced) and patch (absent = unchanged).
#[derive(Debug, Clone, Default)]
pub struct ProviderFields {
    pub slug: Option<String>,
    pub display_name: Option<String>,
    pub issuer_url: Option<String>,
    pub client_id: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub groups_claim: Option<String>,
    pub allowed_email_domains: Option<Vec<String>>,
    pub jit_provisioning: Option<bool>,
    pub allow_email_linking: Option<bool>,
    pub role_sync_mode: Option<String>,
    pub enabled: Option<bool>,
}

pub fn validate_slug(slug: &str) -> Result<String, SsoAdminError> {
    let bytes = slug.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 49
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    if valid {
        Ok(slug.to_string())
    } else {
        Err(SsoAdminError::invalid(
            "slug must match ^[a-z0-9][a-z0-9-]{0,48}$",
        ))
    }
}

fn validate_text(field: &str, value: &str) -> Result<String, SsoAdminError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SsoAdminError::invalid(format!("{field} is required")));
    }
    if trimmed.chars().count() > MAX_TEXT_LEN {
        return Err(SsoAdminError::invalid(format!(
            "{field} must be at most {MAX_TEXT_LEN} characters"
        )));
    }
    Ok(trimmed.to_string())
}

pub fn validate_issuer_url(value: &str) -> Result<String, SsoAdminError> {
    let trimmed = validate_text("issuerUrl", value)?;
    let url = reqwest::Url::parse(&trimmed)
        .map_err(|_| SsoAdminError::invalid("issuerUrl must be a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(SsoAdminError::invalid(
            "issuerUrl must be an http or https URL",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(SsoAdminError::invalid(
            "issuerUrl must not contain a query or fragment",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SsoAdminError::invalid(
            "issuerUrl must not contain credentials",
        ));
    }
    Ok(trimmed)
}

fn validate_scopes(scopes: Vec<String>) -> Result<Vec<String>, SsoAdminError> {
    let mut cleaned: Vec<String> = Vec::new();
    for scope in scopes {
        let scope = scope.trim();
        if scope.is_empty() || scope.chars().any(char::is_whitespace) {
            return Err(SsoAdminError::invalid(
                "scopes must be non-empty values without whitespace",
            ));
        }
        if !cleaned.iter().any(|existing| existing == scope) {
            cleaned.push(scope.to_string());
        }
    }
    if !cleaned.iter().any(|scope| scope == "openid") {
        return Err(SsoAdminError::invalid("scopes must include openid"));
    }
    Ok(cleaned)
}

fn validate_domains(domains: Vec<String>) -> Result<Vec<String>, SsoAdminError> {
    let mut cleaned: Vec<String> = Vec::new();
    for domain in domains {
        let domain = domain.trim().to_ascii_lowercase();
        let valid = !domain.is_empty()
            && !domain.contains('@')
            && !domain.chars().any(char::is_whitespace)
            && domain.len() <= MAX_TEXT_LEN;
        if !valid {
            return Err(SsoAdminError::invalid(
                "allowedEmailDomains must be domain names such as example.com",
            ));
        }
        if !cleaned.contains(&domain) {
            cleaned.push(domain);
        }
    }
    Ok(cleaned)
}

fn validate_role_sync_mode(mode: &str) -> Result<String, SsoAdminError> {
    if ROLE_SYNC_MODES.contains(&mode) {
        Ok(mode.to_string())
    } else {
        Err(SsoAdminError::invalid(
            "roleSyncMode must be one of authoritative, additive, off",
        ))
    }
}

fn require(field: &str, value: Option<String>) -> Result<String, SsoAdminError> {
    value.ok_or_else(|| SsoAdminError::invalid(format!("{field} is required")))
}

/// Validates create input and applies the documented defaults.
pub fn validate_create(fields: ProviderFields) -> Result<ValidatedProvider, SsoAdminError> {
    let slug = validate_slug(&require("slug", fields.slug)?)?;
    let display_name = validate_text("displayName", &require("displayName", fields.display_name)?)?;
    let issuer_url = validate_issuer_url(&require("issuerUrl", fields.issuer_url)?)?;
    let client_id = validate_text("clientId", &require("clientId", fields.client_id)?)?;
    let scopes = match fields.scopes {
        Some(scopes) => validate_scopes(scopes)?,
        None => DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
    };
    let groups_claim = match fields.groups_claim {
        Some(claim) => validate_text("groupsClaim", &claim)?,
        None => DEFAULT_GROUPS_CLAIM.to_string(),
    };
    let allowed_email_domains = validate_domains(fields.allowed_email_domains.unwrap_or_default())?;
    let role_sync_mode = match fields.role_sync_mode {
        Some(mode) => validate_role_sync_mode(&mode)?,
        None => ROLE_SYNC_AUTHORITATIVE.to_string(),
    };
    Ok(ValidatedProvider {
        slug,
        display_name,
        issuer_url,
        client_id,
        scopes,
        groups_claim,
        allowed_email_domains,
        jit_provisioning: fields.jit_provisioning.unwrap_or(true),
        allow_email_linking: fields.allow_email_linking.unwrap_or(false),
        role_sync_mode,
        enabled: fields.enabled.unwrap_or(false),
    })
}

/// Validates the fields present in a patch; absent fields stay `None`.
pub fn validate_patch(fields: ProviderFields) -> Result<ProviderFields, SsoAdminError> {
    Ok(ProviderFields {
        slug: fields.slug.as_deref().map(validate_slug).transpose()?,
        display_name: fields
            .display_name
            .as_deref()
            .map(|v| validate_text("displayName", v))
            .transpose()?,
        issuer_url: fields
            .issuer_url
            .as_deref()
            .map(validate_issuer_url)
            .transpose()?,
        client_id: fields
            .client_id
            .as_deref()
            .map(|v| validate_text("clientId", v))
            .transpose()?,
        scopes: fields.scopes.map(validate_scopes).transpose()?,
        groups_claim: fields
            .groups_claim
            .as_deref()
            .map(|v| validate_text("groupsClaim", v))
            .transpose()?,
        allowed_email_domains: fields
            .allowed_email_domains
            .map(validate_domains)
            .transpose()?,
        jit_provisioning: fields.jit_provisioning,
        allow_email_linking: fields.allow_email_linking,
        role_sync_mode: fields
            .role_sync_mode
            .as_deref()
            .map(validate_role_sync_mode)
            .transpose()?,
        enabled: fields.enabled,
    })
}

/// Outcome of the connection test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryTestResult {
    pub ok: bool,
    pub issuer: Option<String>,
    pub authorization_endpoint: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DiscoveryDocument {
    issuer: Option<String>,
    authorization_endpoint: Option<String>,
    jwks_uri: Option<String>,
}

/// Issuer URLs compare equal ignoring trailing slashes.
pub(crate) fn same_issuer(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The discovery document URL of an issuer.
pub fn discovery_url(issuer_url: &str) -> String {
    format!(
        "{}/.well-known/openid-configuration",
        issuer_url.trim_end_matches('/')
    )
}

/// Fetches the discovery document and the JWKS of `issuer_url` (5 second timeout per
/// request) and checks that the document names the configured issuer. Never fails:
/// every problem is reported in `error`. No credentials are sent.
pub async fn test_discovery(issuer_url: &str) -> DiscoveryTestResult {
    let mut result = DiscoveryTestResult {
        ok: false,
        issuer: None,
        authorization_endpoint: None,
        error: None,
    };
    match run_discovery_test(issuer_url, &mut result).await {
        Ok(()) => result.ok = true,
        Err(message) => result.error = Some(message),
    }
    result
}

async fn run_discovery_test(
    issuer_url: &str,
    result: &mut DiscoveryTestResult,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not create HTTP client".to_string())?;

    let discovery_endpoint = discovery_url(issuer_url);
    let document: DiscoveryDocument = get_json(&client, &discovery_endpoint)
        .await
        .map_err(|err| describe("Discovery", err))?;

    result.issuer = document.issuer.clone();
    result.authorization_endpoint = document.authorization_endpoint.clone();

    let issuer = document
        .issuer
        .as_deref()
        .ok_or_else(|| "Discovery document has no issuer".to_string())?;
    if !same_issuer(issuer, issuer_url) {
        return Err(format!(
            "Issuer mismatch: discovery document reports '{issuer}' but '{issuer_url}' is configured"
        ));
    }
    if document.authorization_endpoint.is_none() {
        return Err("Discovery document has no authorization_endpoint".to_string());
    }
    let jwks_uri = document
        .jwks_uri
        .ok_or_else(|| "Discovery document has no jwks_uri".to_string())?;

    let jwks: serde_json::Value = get_json(&client, &jwks_uri)
        .await
        .map_err(|err| describe("JWKS", err))?;
    match jwks.get("keys").and_then(|keys| keys.as_array()) {
        Some(keys) if !keys.is_empty() => Ok(()),
        _ => Err("JWKS response contains no keys".to_string()),
    }
}

/// Short, URL-free description of a failed discovery or JWKS request.
fn describe(what: &str, err: FetchError) -> String {
    match err {
        FetchError::Status(code) => format!("{what} endpoint returned HTTP {code}"),
        FetchError::InvalidJson if what == "Discovery" => {
            "Discovery document is not valid JSON".to_string()
        }
        FetchError::InvalidJson => format!("{what} response is not valid JSON"),
        FetchError::TooLarge => format!("{what} response exceeds 1 MiB"),
        other => {
            let name = if what == "Discovery" {
                "discovery"
            } else {
                what
            };
            format!("Could not reach {name} endpoint: {other}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn required() -> ProviderFields {
        ProviderFields {
            slug: Some("okta".into()),
            display_name: Some("Okta".into()),
            issuer_url: Some("https://example.okta.com".into()),
            client_id: Some("client".into()),
            ..Default::default()
        }
    }

    #[test]
    fn env_var_name_uses_uppercase_with_underscores() {
        assert_eq!(
            client_secret_env_var("my-idp-2"),
            "FLUXGATE_SSO_MY_IDP_2_CLIENT_SECRET"
        );
        assert_eq!(
            client_secret_env_var("okta"),
            "FLUXGATE_SSO_OKTA_CLIENT_SECRET"
        );
    }

    #[test]
    fn slug_rules() {
        for ok in ["a", "0", "okta", "my-idp-2", &"a".repeat(49)] {
            assert!(validate_slug(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-a", "A", "a_b", "a b", "a/b", &"a".repeat(50), "ü"] {
            assert!(validate_slug(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn create_applies_defaults() {
        let v = validate_create(required()).unwrap();
        assert_eq!(v.scopes, vec!["openid", "email", "profile"]);
        assert_eq!(v.groups_claim, "groups");
        assert!(v.allowed_email_domains.is_empty());
        assert!(v.jit_provisioning);
        assert!(!v.allow_email_linking);
        assert_eq!(v.role_sync_mode, "authoritative");
        assert!(!v.enabled);
    }

    #[test]
    fn create_requires_core_fields() {
        for field in ["slug", "displayName", "issuerUrl", "clientId"] {
            let mut f = required();
            match field {
                "slug" => f.slug = None,
                "displayName" => f.display_name = None,
                "issuerUrl" => f.issuer_url = None,
                _ => f.client_id = None,
            }
            let err = validate_create(f).unwrap_err();
            assert!(err.to_string().contains(field), "{err}");
        }
    }

    #[test]
    fn issuer_url_must_be_plain_http_or_https() {
        for bad in [
            "",
            "not a url",
            "ftp://x.example",
            "https://u:p@x.example",
            "https://x.example/?a=1",
            "https://x.example/#f",
            "javascript:alert(1)",
        ] {
            assert!(validate_issuer_url(bad).is_err(), "{bad}");
        }
        assert!(validate_issuer_url("http://127.0.0.1:9000/realm").is_ok());
        assert!(validate_issuer_url("https://login.example.com/").is_ok());
    }

    #[test]
    fn scopes_must_include_openid_and_be_clean() {
        assert!(validate_scopes(vec!["email".into()]).is_err());
        assert!(validate_scopes(vec!["openid".into(), "a b".into()]).is_err());
        assert!(validate_scopes(vec!["openid".into(), "".into()]).is_err());
        assert_eq!(
            validate_scopes(vec!["openid".into(), "email".into(), "openid".into()]).unwrap(),
            vec!["openid", "email"]
        );
    }

    #[test]
    fn domains_are_normalized() {
        assert_eq!(
            validate_domains(vec![" Example.COM ".into(), "example.com".into()]).unwrap(),
            vec!["example.com"]
        );
        assert!(validate_domains(vec!["a@example.com".into()]).is_err());
        assert!(validate_domains(vec!["".into()]).is_err());
    }

    #[test]
    fn role_sync_mode_is_restricted() {
        for ok in ["authoritative", "additive", "off"] {
            assert!(validate_role_sync_mode(ok).is_ok());
        }
        assert!(validate_role_sync_mode("everything").is_err());
    }

    #[test]
    fn patch_validates_only_present_fields() {
        let patch = validate_patch(ProviderFields {
            display_name: Some(" New ".into()),
            enabled: Some(true),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(patch.display_name.as_deref(), Some("New"));
        assert_eq!(patch.slug, None);
        assert_eq!(patch.enabled, Some(true));
        assert!(
            validate_patch(ProviderFields {
                role_sync_mode: Some("x".into()),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn secrets_without_key_report_encryption_key_missing() {
        let secrets = SsoSecrets::disabled();
        assert!(matches!(
            secrets.seal(Uuid::new_v4(), "s"),
            Err(SsoAdminError::EncryptionKeyMissing)
        ));
    }

    #[test]
    fn sealed_secret_is_bound_to_provider_id() {
        let key = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [3u8; 32]);
        let secrets = SsoSecrets::with_box(SecretBox::from_base64_key(&key).unwrap());
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let sealed = secrets.seal(a, "hunter2").unwrap();
        assert_eq!(secrets.open(a, &sealed).unwrap(), "hunter2");
        assert_eq!(
            secrets.open(b, &sealed),
            Err(SecretBoxError::DecryptionFailed)
        );
    }

    #[test]
    fn issuer_comparison_tolerates_trailing_slash() {
        assert!(same_issuer("https://a.example/", "https://a.example"));
        assert!(same_issuer("https://a.example/x", "https://a.example/x/"));
        assert!(!same_issuer("https://a.example/x", "https://a.example/y"));
    }
}
