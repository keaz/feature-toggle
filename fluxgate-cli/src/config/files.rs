//! The INI config and credentials files.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use ini::Ini;

use super::Paths;
use crate::error::CliError;

/// Keys a profile section may hold.
pub const PROFILE_KEYS: [&str; 7] = [
    "session",
    "url",
    "team",
    "environment",
    "output",
    "timeout",
    "edge_url",
];

pub fn profile_section(profile: &str) -> String {
    if profile == "default" {
        "default".to_string()
    } else {
        format!("profile {profile}")
    }
}

pub fn session_section(session: &str) -> String {
    format!("session {session}")
}

pub struct ConfigFiles {
    pub config: Ini,
    pub credentials: Ini,
    /// Problems worth telling the user about, such as loose file permissions.
    pub warnings: Vec<String>,
}

impl ConfigFiles {
    pub fn empty() -> Self {
        Self {
            config: Ini::new(),
            credentials: Ini::new(),
            warnings: Vec::new(),
        }
    }

    /// Missing files load as empty.
    pub fn load(paths: &Paths) -> Result<Self, CliError> {
        Ok(Self {
            config: load_ini(&paths.config)?,
            credentials: load_ini(&paths.credentials)?,
            warnings: permission_warning(&paths.credentials).into_iter().collect(),
        })
    }

    pub fn profile_value(&self, profile: &str, key: &str) -> Option<&str> {
        value_of(&self.config, profile_section(profile), key)
    }

    pub fn session_value(&self, session: &str, key: &str) -> Option<&str> {
        value_of(&self.config, session_section(session), key)
    }

    pub fn credential_token(&self, profile: &str) -> Option<&str> {
        self.credential_value(profile, "token")
    }

    pub fn credential_value(&self, profile: &str, key: &str) -> Option<&str> {
        value_of(&self.credentials, profile.to_string(), key)
    }

    /// Profiles in either file: `default` first, then sorted.
    pub fn profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .config
            .sections()
            .flatten()
            .filter_map(|section| {
                if section == "default" {
                    Some("default".to_string())
                } else {
                    section
                        .strip_prefix("profile ")
                        .map(|name| name.trim().to_string())
                }
            })
            .collect();
        names.extend(
            self.credentials
                .sections()
                .flatten()
                .map(|s| s.trim().to_string()),
        );
        names.sort();
        names.dedup();
        if let Some(position) = names.iter().position(|name| name == "default") {
            let default = names.remove(position);
            names.insert(0, default);
        }
        names
    }

    pub fn has_profile(&self, profile: &str) -> bool {
        self.profile_names().iter().any(|name| name == profile)
    }

    pub fn set_profile_value(&mut self, profile: &str, key: &str, value: &str) {
        self.config
            .with_section(Some(profile_section(profile)))
            .set(key, value);
    }

    pub fn remove_profile_value(&mut self, profile: &str, key: &str) {
        self.config.delete_from(Some(profile_section(profile)), key);
    }

    pub fn remove_session_value(&mut self, session: &str, key: &str) {
        self.config.delete_from(Some(session_section(session)), key);
    }

    pub fn set_session_value(&mut self, session: &str, key: &str, value: &str) {
        self.config
            .with_section(Some(session_section(session)))
            .set(key, value);
    }

    pub fn set_credential_token(&mut self, profile: &str, token: &str) {
        self.set_credential_value(profile, "token", token);
    }

    pub fn set_credential_value(&mut self, profile: &str, key: &str, value: &str) {
        self.credentials.with_section(Some(profile)).set(key, value);
    }

    pub fn save_config(&self, paths: &Paths) -> Result<(), CliError> {
        save_ini(&self.config, &paths.config)
    }

    pub fn save_credentials(&self, paths: &Paths) -> Result<(), CliError> {
        save_ini(&self.credentials, &paths.credentials)
    }
}

fn value_of<'a>(ini: &'a Ini, section: String, key: &str) -> Option<&'a str> {
    ini.section(Some(section))
        .and_then(|properties| properties.get(key))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn load_ini(path: &Path) -> Result<Ini, CliError> {
    if !path.exists() {
        return Ok(Ini::new());
    }
    Ini::load_from_file(path)
        .map(normalize_sections)
        .map_err(|err| CliError::Usage(format!("cannot read {}: {err}", path.display())))
}

/// `[profile   prod]` becomes `[profile prod]`, so lookups find what
/// `profile_names` lists instead of silently using defaults.
fn normalize_sections(ini: Ini) -> Ini {
    let mut normalized = Ini::new();
    for (section, properties) in ini.iter() {
        let name = section.map(|name| name.split_whitespace().collect::<Vec<_>>().join(" "));
        for (key, value) in properties.iter() {
            normalized.with_section(name.clone()).set(key, value);
        }
    }
    normalized
}

fn save_ini(ini: &Ini, path: &Path) -> Result<(), CliError> {
    let mut buffer = Vec::new();
    ini.write_to(&mut buffer)?;
    write_private(path, &buffer)
}

/// Writes `contents` to `path` with mode 0600, creating parent directories.
///
/// The data goes to a temporary file that is then renamed over `path`, so a
/// reader sees the old or the new file, never an empty or partial one.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<(), CliError> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let name = path
        .file_name()
        .ok_or_else(|| CliError::Other(format!("invalid file path {}", path.display())))?;
    let temporary = path.with_file_name(format!(
        ".{}.tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result =
        write_new_private(&temporary, contents).and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(CliError::from)
}

/// Distinguishes temporary files written by one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn write_new_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// A warning when group or others can read `path`.
pub fn permission_warning(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode();
        if mode & 0o077 != 0 {
            return Some(format!(
                "warning: {} is readable by other users; run chmod 600 {}",
                path.display(),
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn paths(dir: &Path) -> Paths {
        Paths {
            config: dir.join("config"),
            credentials: dir.join("credentials"),
            sessions: dir.join("sessions"),
        }
    }

    #[test]
    fn missing_files_load_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let files = ConfigFiles::load(&paths(dir.path())).unwrap();
        assert!(files.profile_names().is_empty());
        assert!(files.warnings.is_empty());
    }

    #[test]
    fn reads_default_and_named_profiles_sessions_and_tokens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config"),
            "[default]\nteam = payments\n\n[profile prod]\nsession = corp\nteam = checkout\n\n[session corp]\nurl = https://fg.example.com/api/v1\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("credentials"), "[ci]\ntoken = abc\n").unwrap();
        let files = ConfigFiles::load(&paths(dir.path())).unwrap();
        assert_eq!(files.profile_value("default", "team"), Some("payments"));
        assert_eq!(files.profile_value("prod", "team"), Some("checkout"));
        assert_eq!(files.profile_value("prod", "missing"), None);
        assert_eq!(
            files.session_value("corp", "url"),
            Some("https://fg.example.com/api/v1")
        );
        assert_eq!(files.credential_token("ci"), Some("abc"));
        assert_eq!(files.profile_names(), vec!["default", "ci", "prod"]);
        assert!(files.has_profile("ci"));
        assert!(!files.has_profile("nope"));
    }

    #[test]
    fn save_keeps_unknown_keys_and_adds_new_values() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(
            &p.config,
            "[profile prod]\nteam = checkout\ncustom_key = keep-me\n",
        )
        .unwrap();
        let mut files = ConfigFiles::load(&p).unwrap();
        files.set_profile_value("prod", "environment", "production");
        files.set_session_value("corp", "url", "https://fg.example.com/api/v1");
        files.save_config(&p).unwrap();
        let again = ConfigFiles::load(&p).unwrap();
        assert_eq!(again.profile_value("prod", "custom_key"), Some("keep-me"));
        assert_eq!(
            again.profile_value("prod", "environment"),
            Some("production")
        );
        assert_eq!(
            again.session_value("corp", "url"),
            Some("https://fg.example.com/api/v1")
        );
    }

    #[test]
    fn remove_profile_value_deletes_the_key() {
        let mut files = ConfigFiles::empty();
        files.set_profile_value("default", "url", "u");
        files.remove_profile_value("default", "url");
        assert_eq!(files.profile_value("default", "url"), None);
    }

    #[cfg(unix)]
    #[test]
    fn saved_files_are_private_and_parent_dirs_are_created() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = paths(&dir.path().join("nested"));
        let mut files = ConfigFiles::empty();
        files.set_credential_token("ci", "secret");
        files.save_credentials(&p).unwrap();
        let mode = std::fs::metadata(&p.credentials)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn warns_when_credentials_are_readable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.credentials, "[ci]\ntoken = abc\n").unwrap();
        std::fs::set_permissions(&p.credentials, std::fs::Permissions::from_mode(0o644)).unwrap();
        let files = ConfigFiles::load(&p).unwrap();
        assert_eq!(files.warnings.len(), 1);
        assert!(files.warnings[0].contains("chmod 600"));
    }

    #[test]
    fn unreadable_ini_is_a_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.config, "[unclosed\n").unwrap();
        let err = ConfigFiles::load(&p).err().unwrap();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
    }

    #[test]
    fn write_private_replaces_the_file_instead_of_rewriting_it() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions").join("corp.json");
        write_private(&path, b"old contents").unwrap();
        // A reader that opened the file before the write must still see the
        // complete old contents, never an empty or partial file.
        let mut reader = std::fs::File::open(&path).unwrap();
        write_private(&path, b"new").unwrap();
        let mut seen = String::new();
        reader.read_to_string(&mut seen).unwrap();
        assert_eq!(seen, "old contents");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temporary file left behind");
    }

    #[test]
    fn section_names_with_extra_spaces_are_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.config, "[profile   prod]\nteam = checkout\n\n[session  corp ]\nurl = https://fg.example.com/api/v1\n").unwrap();
        let files = ConfigFiles::load(&p).unwrap();
        assert_eq!(files.profile_value("prod", "team"), Some("checkout"));
        assert_eq!(
            files.session_value("corp", "url"),
            Some("https://fg.example.com/api/v1")
        );
    }
}
