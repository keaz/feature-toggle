//! Device authorization for `fluxgate login --use-device-code`: the CLI gets a
//! device code and a short user code, a signed-in user approves the user code in
//! the web UI, and the CLI's polling then receives a session.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::Rng;

pub use crate::database::entity::CliDeviceAuthorization;

/// How long a device code can be approved and polled.
pub const DEVICE_CODE_TTL_MINUTES: i64 = 10;
/// Minimum seconds between two polls of one device code.
pub const POLL_INTERVAL_SECS: i32 = 5;
/// Consonants without lookalikes (no L, no vowels, so no words).
pub const USER_CODE_ALPHABET: &str = "BCDFGHJKMNPQRSTVWXZ";

/// `XXXX-XXXX` from [`USER_CODE_ALPHABET`].
pub fn new_user_code() -> String {
    let alphabet: Vec<char> = USER_CODE_ALPHABET.chars().collect();
    let mut rng = rand::rng();
    let mut pick = || alphabet[rng.random_range(0..alphabet.len())];
    let first: String = (0..4).map(|_| pick()).collect();
    let second: String = (0..4).map(|_| pick()).collect();
    format!("{first}-{second}")
}

/// 32 random bytes, base64url without padding.
pub fn new_device_code() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The canonical `XXXX-XXXX` form of a code as a person typed it (any case,
/// spaces or hyphen optional), or `None` when it cannot be a user code.
pub fn normalize_user_code(typed: &str) -> Option<String> {
    let letters: String = typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (letters.len() == 8 && letters.chars().all(|c| USER_CODE_ALPHABET.contains(c)))
        .then(|| format!("{}-{}", &letters[..4], &letters[4..]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    Pending,
    SlowDown,
    Approved,
    Denied,
    /// Expired, consumed, or never issued.
    Expired,
}

/// What a poll of `row` at `now` answers.
pub fn poll_outcome(row: &CliDeviceAuthorization, now: DateTime<Utc>) -> PollOutcome {
    if row.expires_at <= now {
        return PollOutcome::Expired;
    }
    match row.status.as_str() {
        "approved" => PollOutcome::Approved,
        "denied" => PollOutcome::Denied,
        "pending" => {
            let too_soon = row.last_polled_at.is_some_and(|at| {
                now - at < chrono::Duration::seconds(i64::from(row.interval_secs))
            });
            if too_soon {
                PollOutcome::SlowDown
            } else {
                PollOutcome::Pending
            }
        }
        _ => PollOutcome::Expired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    #[test]
    fn user_codes_are_two_groups_of_four_unambiguous_letters() {
        for _ in 0..200 {
            let code = new_user_code();
            assert_eq!(code.len(), 9, "{code}");
            assert_eq!(&code[4..5], "-", "{code}");
            assert!(
                code.chars()
                    .filter(|c| *c != '-')
                    .all(|c| USER_CODE_ALPHABET.contains(c)),
                "{code}"
            );
        }
        for lookalike in ['O', 'I', 'L', '0', '1', 'A', 'E', 'U'] {
            assert!(!USER_CODE_ALPHABET.contains(lookalike), "{lookalike}");
        }
    }

    #[test]
    fn user_codes_are_normalized_from_what_people_type() {
        assert_eq!(
            normalize_user_code(" bcdf ghjk ").as_deref(),
            Some("BCDF-GHJK")
        );
        assert_eq!(
            normalize_user_code("bcdf-ghjk").as_deref(),
            Some("BCDF-GHJK")
        );
        assert_eq!(
            normalize_user_code("BCDFGHJK").as_deref(),
            Some("BCDF-GHJK")
        );
        assert_eq!(normalize_user_code("BCDF-GHJ"), None);
        assert_eq!(normalize_user_code("BCDF-GHJ1"), None);
        assert_eq!(normalize_user_code("BCDF-GHJKM"), None);
    }

    #[test]
    fn device_codes_are_long_random_url_safe_strings() {
        let code = new_device_code();
        assert_eq!(code.len(), 43);
        assert!(
            code.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(code, new_device_code());
    }

    fn row(
        status: &str,
        polled_secs_ago: Option<i64>,
        expires_in_secs: i64,
    ) -> CliDeviceAuthorization {
        let now = Utc::now();
        CliDeviceAuthorization {
            id: Uuid::new_v4(),
            device_code_hash: "h".into(),
            user_code: "BCDF-GHJK".into(),
            status: status.into(),
            user_id: None,
            interval_secs: 5,
            last_polled_at: polled_secs_ago.map(|secs| now - Duration::seconds(secs)),
            expires_at: now + Duration::seconds(expires_in_secs),
            created_at: now,
        }
    }

    #[test]
    fn poll_outcomes_follow_status_expiry_and_interval() {
        let now = Utc::now();
        assert_eq!(
            poll_outcome(&row("pending", None, 600), now),
            PollOutcome::Pending
        );
        assert_eq!(
            poll_outcome(&row("pending", Some(2), 600), now),
            PollOutcome::SlowDown
        );
        assert_eq!(
            poll_outcome(&row("pending", Some(6), 600), now),
            PollOutcome::Pending
        );
        assert_eq!(
            poll_outcome(&row("approved", Some(1), 600), now),
            PollOutcome::Approved
        );
        assert_eq!(
            poll_outcome(&row("denied", None, 600), now),
            PollOutcome::Denied
        );
        assert_eq!(
            poll_outcome(&row("consumed", None, 600), now),
            PollOutcome::Expired
        );
        assert_eq!(
            poll_outcome(&row("approved", None, -1), now),
            PollOutcome::Expired
        );
        assert_eq!(
            poll_outcome(&row("pending", None, -1), now),
            PollOutcome::Expired
        );
    }
}
