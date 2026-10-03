//! Validation for links between a feature and issues in an external tracker.

use std::sync::LazyLock;

use regex::Regex;

use crate::Error;

/// The only external system supported today.
pub const SYSTEM_JIRA: &str = "jira";

/// Longest accepted link URL.
pub const MAX_URL_LENGTH: usize = 2048;

static JIRA_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]+-[1-9][0-9]*$").expect("valid Jira key regex"));

/// Normalizes a Jira issue key: trims it, upper-cases it and checks it matches
/// `^[A-Z][A-Z0-9_]+-[1-9][0-9]*$`.
pub fn normalize_jira_key(input: &str) -> Result<String, Error> {
    let key = input.trim().to_uppercase();
    if JIRA_KEY.is_match(&key) {
        Ok(key)
    } else {
        Err(Error::InvalidInput(
            "externalKey must be a Jira issue key such as PROJ-123".to_string(),
        ))
    }
}

/// Validates an optional link URL: blank is `None`; otherwise `http` or `https`
/// and at most [`MAX_URL_LENGTH`] characters.
pub fn validate_url(input: Option<&str>) -> Result<Option<String>, Error> {
    let Some(url) = input.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if url.chars().count() > MAX_URL_LENGTH {
        return Err(Error::InvalidInput(format!(
            "url must be at most {MAX_URL_LENGTH} characters"
        )));
    }
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| Error::InvalidInput("url must be a valid URL".to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(Error::InvalidInput(
            "url must be an http or https URL".to_string(),
        ));
    }
    Ok(Some(url.to_string()))
}

/// A link request that passed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewExternalLink {
    pub system: String,
    pub external_key: String,
    pub url: Option<String>,
}

/// Validates a link request: `system` must be `jira` (any case), the key a Jira
/// issue key, the URL optional `http(s)`.
pub fn validate_new_link(
    system: &str,
    external_key: &str,
    url: Option<&str>,
) -> Result<NewExternalLink, Error> {
    let system = system.trim().to_lowercase();
    if system != SYSTEM_JIRA {
        return Err(Error::InvalidInput(format!(
            "system must be '{SYSTEM_JIRA}'"
        )));
    }
    Ok(NewExternalLink {
        system,
        external_key: normalize_jira_key(external_key)?,
        url: validate_url(url)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_new_link_normalizes_system_key_and_url() {
        let link = validate_new_link(
            " Jira ",
            "proj-7",
            Some(" https://jira.example.com/browse/PROJ-7 "),
        )
        .expect("valid link");
        assert_eq!(
            link,
            NewExternalLink {
                system: SYSTEM_JIRA.to_string(),
                external_key: "PROJ-7".to_string(),
                url: Some("https://jira.example.com/browse/PROJ-7".to_string()),
            }
        );
        assert_eq!(validate_new_link("jira", "PROJ-7", None).unwrap().url, None);
    }

    #[test]
    fn validate_new_link_rejects_unknown_system_bad_key_and_bad_url() {
        for (system, key, url) in [
            ("github", "PROJ-7", None),
            ("", "PROJ-7", None),
            ("jira", "PROJ", None),
            ("jira", "PROJ-7", Some("ftp://jira.example.com")),
        ] {
            assert!(
                matches!(
                    validate_new_link(system, key, url),
                    Err(Error::InvalidInput(_))
                ),
                "{system:?} {key:?} {url:?} should be rejected"
            );
        }
    }

    #[test]
    fn normalize_jira_key_accepts_and_normalizes_valid_keys() {
        let cases = [
            ("PROJ-123", "PROJ-123"),
            ("proj-123", "PROJ-123"),
            (" ABC_1-9 ", "ABC_1-9"),
            ("AB-1", "AB-1"),
            ("A1-10", "A1-10"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_jira_key(input).unwrap_or_else(|err| panic!("{input:?}: {err}")),
                expected,
                "input {input:?}"
            );
        }
    }

    #[test]
    fn normalize_jira_key_rejects_invalid_keys() {
        let cases = [
            "", "   ", "PROJ-0", "123-4", "PROJ", "PROJ-12a", "P-1", "PROJ-01", "PROJ 1-2",
            "_AB-1", "PROJ--1", "PROJ-1-2",
        ];
        for input in cases {
            assert!(
                matches!(normalize_jira_key(input), Err(Error::InvalidInput(_))),
                "input {input:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_url_treats_missing_and_blank_as_none() {
        assert_eq!(validate_url(None).unwrap(), None);
        assert_eq!(validate_url(Some("")).unwrap(), None);
        assert_eq!(validate_url(Some("   ")).unwrap(), None);
    }

    #[test]
    fn validate_url_accepts_http_and_https() {
        let cases = [
            (
                "https://acme.atlassian.net/browse/PROJ-1",
                "https://acme.atlassian.net/browse/PROJ-1",
            ),
            (
                " http://jira.local:8080/browse/PROJ-1 ",
                "http://jira.local:8080/browse/PROJ-1",
            ),
            ("HTTPS://jira.example.com/x", "HTTPS://jira.example.com/x"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                validate_url(Some(input)).unwrap_or_else(|err| panic!("{input:?}: {err}")),
                Some(expected.to_string()),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn validate_url_rejects_other_schemes_and_long_urls() {
        let too_long = format!("https://jira.example.com/{}", "a".repeat(MAX_URL_LENGTH));
        let cases = [
            "javascript:alert(1)",
            "ftp://jira.example.com/x",
            "jira.example.com/browse/PROJ-1",
            "https://",
            "data:text/html,hi",
            too_long.as_str(),
        ];
        for input in cases {
            assert!(
                matches!(validate_url(Some(input)), Err(Error::InvalidInput(_))),
                "input {input:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_url_accepts_exactly_max_length() {
        let prefix = "https://jira.example.com/";
        let url = format!("{prefix}{}", "a".repeat(MAX_URL_LENGTH - prefix.len()));
        assert_eq!(url.len(), MAX_URL_LENGTH);
        assert_eq!(validate_url(Some(&url)).unwrap(), Some(url.clone()));
    }
}
