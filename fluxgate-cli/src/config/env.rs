//! Snapshot of the process environment, so resolution is testable.

use std::collections::HashMap;
use std::ffi::OsString;

#[derive(Debug, Clone, Default)]
pub struct Env(HashMap<String, String>);

impl Env {
    pub fn from_process() -> Self {
        Self::from_os_pairs(std::env::vars_os())
    }

    /// Pairs that are not valid Unicode are skipped instead of panicking:
    /// an unrelated variable must not stop the CLI.
    pub fn from_os_pairs(pairs: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        Self(
            pairs
                .into_iter()
                .filter_map(|(key, value)| {
                    Some((key.into_string().ok()?, value.into_string().ok()?))
                })
                .collect(),
        )
    }

    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
    }

    /// Trimmed value of `key`; blank values count as unset.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.first(&[key])
    }

    /// Value of the first key in `keys` that is set and not blank.
    pub fn first(&self, keys: &[&str]) -> Option<&str> {
        keys.iter()
            .filter_map(|key| self.0.get(*key))
            .map(|value| value.trim())
            .find(|value| !value.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_skips_unset_and_blank_values() {
        let env = Env::from_pairs([("A", "  "), ("B", " value ")]);
        assert_eq!(env.get("A"), None);
        assert_eq!(env.get("B"), Some("value"));
        assert_eq!(env.first(&["MISSING", "A", "B"]), Some("value"));
        assert_eq!(env.first(&["MISSING"]), None);
    }

    #[cfg(unix)]
    #[test]
    fn from_os_pairs_skips_values_that_are_not_unicode() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let env = Env::from_os_pairs([
            (OsString::from("GOOD"), OsString::from("yes")),
            (OsString::from("BAD"), OsString::from_vec(vec![0xff, 0xfe])),
        ]);
        assert_eq!(env.get("GOOD"), Some("yes"));
        assert_eq!(env.get("BAD"), None);
    }
}
