//! Configuration: environment snapshot, config and credentials files, resolution.

pub mod env;
pub mod files;
pub mod paths;

pub use env::Env;
pub use files::ConfigFiles;
pub use paths::Paths;

pub mod resolve;

pub use resolve::{Credential, Overrides, Resolved, Settings, Source, resolve, selected_profile};

/// `****` followed by the last 4 characters.
pub fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("****{tail}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn mask_token_keeps_the_last_four_characters() {
        assert_eq!(super::mask_token("abcdef123456"), "****3456");
        assert_eq!(super::mask_token("ab"), "****ab");
    }
}
