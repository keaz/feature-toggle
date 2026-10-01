//! Pure helpers of the SSO login flow: redirect sanitizing, URL building, email
//! domain checks and the derivation of usernames and names for JIT provisioning.

use reqwest::Url;

/// Lifetime of an authorization request (state row).
pub const LOGIN_STATE_TTL_MINUTES: i64 = 10;
/// Lifetime of a one-time exchange code.
pub const LOGIN_CODE_TTL_SECONDS: i64 = 60;
/// Length limit of `users.username`, `users.first_name` and `users.last_name`.
pub const USER_NAME_COLUMN_LIMIT: usize = 100;
/// Length limit of `users.email`.
pub const USER_EMAIL_COLUMN_LIMIT: usize = 255;
/// Longest post-login redirect path that is kept.
const MAX_REDIRECT_LEN: usize = 2048;

/// Why an SSO login failed. [`SsoLoginError::code`] is the `ssoError` value the UI
/// receives; the detail is for the server log only and never contains tokens,
/// codes or secrets.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SsoLoginError {
    #[error("state missing, expired or already used")]
    StateInvalid,
    #[error("identity provider error: {0}")]
    ProviderError(String),
    #[error("id_token invalid: {0}")]
    TokenInvalid(String),
    #[error("no email claim")]
    EmailMissing,
    #[error("email domain not allowed")]
    EmailDomainNotAllowed,
    #[error("user not provisioned and JIT provisioning is off")]
    UserNotProvisioned,
    #[error("an account with this email exists and linking is not allowed")]
    LinkingNotAllowed,
    #[error("account disabled")]
    AccountDisabled,
}

impl SsoLoginError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::StateInvalid => "sso_state_invalid",
            Self::ProviderError(_) => "sso_provider_error",
            Self::TokenInvalid(_) => "sso_token_invalid",
            Self::EmailMissing => "sso_email_missing",
            Self::EmailDomainNotAllowed => "sso_email_domain_not_allowed",
            Self::UserNotProvisioned => "sso_user_not_provisioned",
            Self::LinkingNotAllowed => "sso_linking_not_allowed",
            Self::AccountDisabled => "sso_account_disabled",
        }
    }
}

impl From<crate::Error> for SsoLoginError {
    fn from(err: crate::Error) -> Self {
        SsoLoginError::ProviderError(format!("internal error: {err}"))
    }
}

/// Keeps a post-login redirect only if it is a local path: it must start with `/`
/// and must not start with `//` or `/\` (protocol-relative URLs that browsers send
/// to another host). Control characters, backslashes and overlong values are
/// dropped as well. Anything else is ignored (`None`).
pub fn sanitize_redirect(value: Option<&str>) -> Option<String> {
    let value = value?;
    let local = value.starts_with('/')
        && !value.starts_with("//")
        && !value.starts_with("/\\")
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
        && value.len() <= MAX_REDIRECT_LEN;
    local.then(|| value.to_string())
}

/// `<base>/api/v1/auth/sso/<slug>/callback`.
pub fn callback_url(public_base: &str, slug: &str) -> String {
    format!(
        "{}/api/v1/auth/sso/{slug}/callback",
        public_base.trim_end_matches('/')
    )
}

/// Parameters of the authorization request.
pub struct AuthorizeParams<'a> {
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub scopes: &'a [String],
    pub state: &'a str,
    pub nonce: &'a str,
    pub code_challenge: &'a str,
}

/// The IdP authorization URL (Authorization Code flow with PKCE S256). Existing
/// query parameters of the endpoint are kept.
pub fn authorize_url(endpoint: &str, params: &AuthorizeParams<'_>) -> Result<String, String> {
    let mut url = Url::parse(endpoint).map_err(|_| "invalid authorization_endpoint".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("authorization_endpoint must be http or https".to_string());
    }
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", params.client_id)
        .append_pair("redirect_uri", params.redirect_uri)
        .append_pair("scope", &params.scopes.join(" "))
        .append_pair("state", params.state)
        .append_pair("nonce", params.nonce)
        .append_pair("code_challenge", params.code_challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.to_string())
}

fn ui_url(ui_origin: &str, path: &str, query: &[(&str, &str)]) -> String {
    let base = format!("{}{path}", ui_origin.trim_end_matches('/'));
    match Url::parse(&base) {
        Ok(mut url) => {
            {
                let mut pairs = url.query_pairs_mut();
                for (key, value) in query {
                    pairs.append_pair(key, value);
                }
            }
            url.to_string()
        }
        // An unparsable origin is a configuration error; still never put the raw
        // values together unencoded.
        Err(_) => {
            let encoded = serde_urlencoded::to_string(query).unwrap_or_default();
            format!("{base}?{encoded}")
        }
    }
}

/// `<ui>/auth/sso/complete?code=<code>[&redirect=<path>]`.
pub fn complete_url(ui_origin: &str, code: &str, redirect: Option<&str>) -> String {
    let mut query = vec![("code", code)];
    if let Some(redirect) = redirect {
        query.push(("redirect", redirect));
    }
    ui_url(ui_origin, "/auth/sso/complete", &query)
}

/// `<ui>/login?ssoError=<code>`.
pub fn error_url(ui_origin: &str, error: &SsoLoginError) -> String {
    ui_url(ui_origin, "/login", &[("ssoError", error.code())])
}

/// Whether `email` belongs to one of `allowed_domains` (case-insensitive). An empty
/// list allows every domain.
pub fn email_domain_allowed(email: &str, allowed_domains: &[String]) -> bool {
    if allowed_domains.is_empty() {
        return true;
    }
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    let domain = domain.to_ascii_lowercase();
    allowed_domains
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&domain))
}

/// The first `max` characters of `value`.
pub fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn email_local_part(email: &str) -> &str {
    email.split('@').next().unwrap_or(email)
}

/// Username candidate before uniqueness: `preferred_username`, else the email local
/// part, trimmed, without whitespace or control characters, truncated to the column
/// limit.
pub fn username_base(preferred_username: Option<&str>, email: &str) -> String {
    let clean = |value: &str| -> String {
        value
            .chars()
            .filter(|c| !c.is_whitespace() && !c.is_control())
            .collect()
    };
    let mut base = preferred_username.map(clean).unwrap_or_default();
    if base.is_empty() {
        base = clean(email_local_part(email));
    }
    if base.is_empty() {
        base = "user".to_string();
    }
    truncate_chars(&base, USER_NAME_COLUMN_LIMIT)
}

/// The `n`-th username candidate: `base` for 1, then `base2`, `base3`, ... with the
/// base shortened so the result stays within the column limit.
pub fn username_candidate(base: &str, n: u32) -> String {
    if n <= 1 {
        return truncate_chars(base, USER_NAME_COLUMN_LIMIT);
    }
    let suffix = n.to_string();
    let keep = USER_NAME_COLUMN_LIMIT.saturating_sub(suffix.len());
    format!("{}{suffix}", truncate_chars(base, keep))
}

/// First and last name: `given_name` / `family_name`, else `name` split at the
/// first space, else the email local part and `"-"`. Truncated to the column limit.
pub fn derive_names(
    given_name: Option<&str>,
    family_name: Option<&str>,
    name: Option<&str>,
    email: &str,
) -> (String, String) {
    fn non_empty(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|v| !v.is_empty())
    }
    let (mut first, mut last) = (
        non_empty(given_name).map(str::to_string),
        non_empty(family_name).map(str::to_string),
    );
    if (first.is_none() || last.is_none())
        && let Some(full) = non_empty(name)
    {
        let mut parts = full.splitn(2, char::is_whitespace);
        let name_first = parts.next().map(str::trim).filter(|v| !v.is_empty());
        let name_last = parts.next().map(str::trim).filter(|v| !v.is_empty());
        if first.is_none() {
            first = name_first.map(str::to_string);
        }
        if last.is_none() {
            last = name_last.map(str::to_string);
        }
    }
    let first = first.unwrap_or_else(|| {
        let local = email_local_part(email).trim();
        if local.is_empty() {
            "-".to_string()
        } else {
            local.to_string()
        }
    });
    let last = last.unwrap_or_else(|| "-".to_string());
    (
        truncate_chars(&first, USER_NAME_COLUMN_LIMIT),
        truncate_chars(&last, USER_NAME_COLUMN_LIMIT),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_accepts_only_local_paths() {
        for ok in ["/", "/features", "/features?env=prod#x", "/a//b"] {
            assert_eq!(sanitize_redirect(Some(ok)).as_deref(), Some(ok), "{ok}");
        }
        for bad in [
            "//evil.example",
            "/\\evil.example",
            "https://evil.example",
            "http:/evil",
            "evil",
            "",
            "javascript:alert(1)",
            "/a\\b",
            "/a\nb",
            "/a\tb",
        ] {
            assert_eq!(sanitize_redirect(Some(bad)), None, "{bad:?}");
        }
        assert_eq!(sanitize_redirect(None), None);
        assert_eq!(
            sanitize_redirect(Some(&format!("/{}", "a".repeat(3000)))),
            None
        );
    }

    #[test]
    fn authorize_url_carries_pkce_state_and_nonce() {
        let scopes = vec!["openid".to_string(), "email".to_string()];
        let url = authorize_url(
            "https://idp.example/auth?prompt=login",
            &AuthorizeParams {
                client_id: "client id",
                redirect_uri: "https://app.example/api/v1/auth/sso/okta/callback",
                scopes: &scopes,
                state: "st",
                nonce: "no",
                code_challenge: "ch",
            },
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["prompt"], "login");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], "client id");
        assert_eq!(
            q["redirect_uri"],
            "https://app.example/api/v1/auth/sso/okta/callback"
        );
        assert_eq!(q["scope"], "openid email");
        assert_eq!(q["state"], "st");
        assert_eq!(q["nonce"], "no");
        assert_eq!(q["code_challenge"], "ch");
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(
            authorize_url(
                "javascript:alert(1)",
                &AuthorizeParams {
                    client_id: "c",
                    redirect_uri: "r",
                    scopes: &scopes,
                    state: "s",
                    nonce: "n",
                    code_challenge: "c",
                }
            )
            .is_err()
        );
    }

    #[test]
    fn ui_urls_encode_values() {
        assert_eq!(
            complete_url("http://ui.example/", "abc", Some("/features?x=1&y=2")),
            "http://ui.example/auth/sso/complete?code=abc&redirect=%2Ffeatures%3Fx%3D1%26y%3D2"
        );
        assert_eq!(
            complete_url("http://ui.example", "abc", None),
            "http://ui.example/auth/sso/complete?code=abc"
        );
        assert_eq!(
            error_url("http://ui.example", &SsoLoginError::StateInvalid),
            "http://ui.example/login?ssoError=sso_state_invalid"
        );
    }

    #[test]
    fn error_codes_match_contract() {
        let all = [
            (SsoLoginError::StateInvalid, "sso_state_invalid"),
            (
                SsoLoginError::ProviderError("x".into()),
                "sso_provider_error",
            ),
            (SsoLoginError::TokenInvalid("x".into()), "sso_token_invalid"),
            (SsoLoginError::EmailMissing, "sso_email_missing"),
            (
                SsoLoginError::EmailDomainNotAllowed,
                "sso_email_domain_not_allowed",
            ),
            (
                SsoLoginError::UserNotProvisioned,
                "sso_user_not_provisioned",
            ),
            (SsoLoginError::LinkingNotAllowed, "sso_linking_not_allowed"),
            (SsoLoginError::AccountDisabled, "sso_account_disabled"),
        ];
        for (err, code) in all {
            assert_eq!(err.code(), code);
        }
    }

    #[test]
    fn domain_check_is_case_insensitive() {
        let domains = vec!["example.com".to_string()];
        assert!(email_domain_allowed("a@Example.COM", &domains));
        assert!(!email_domain_allowed("a@example.com.evil", &domains));
        assert!(!email_domain_allowed("a@sub.example.com", &domains));
        assert!(!email_domain_allowed("no-at-sign", &domains));
        assert!(email_domain_allowed("a@anything", &[]));
    }

    #[test]
    fn usernames_fall_back_and_stay_within_limit() {
        assert_eq!(username_base(Some(" jdoe "), "x@y"), "jdoe");
        assert_eq!(username_base(None, "jane.doe@example.com"), "jane.doe");
        assert_eq!(username_base(Some("  "), "jane@example.com"), "jane");
        assert_eq!(username_base(Some("j doe"), "x@y"), "jdoe");
        let long = "u".repeat(150);
        assert_eq!(username_base(Some(&long), "x@y").chars().count(), 100);
        assert_eq!(username_candidate("jdoe", 1), "jdoe");
        assert_eq!(username_candidate("jdoe", 2), "jdoe2");
        let c = username_candidate(&"u".repeat(100), 12);
        assert_eq!(c.chars().count(), 100);
        assert!(c.ends_with("12"));
    }

    #[test]
    fn names_follow_fallback_order() {
        assert_eq!(
            derive_names(Some("Jane"), Some("Doe"), Some("X Y"), "j@e"),
            ("Jane".into(), "Doe".into())
        );
        assert_eq!(
            derive_names(None, None, Some("Jane van Doe"), "j@e"),
            ("Jane".into(), "van Doe".into())
        );
        assert_eq!(
            derive_names(None, None, Some("Mononym"), "j@e"),
            ("Mononym".into(), "-".into())
        );
        assert_eq!(
            derive_names(None, None, None, "jane@example.com"),
            ("jane".into(), "-".into())
        );
        assert_eq!(
            derive_names(Some(&"a".repeat(200)), None, None, "j@e")
                .0
                .len(),
            100
        );
    }
}
