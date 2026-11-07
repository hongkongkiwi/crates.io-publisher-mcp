use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Directory for credentials stored by this tool.
///
/// `CRATES_MCP_CONFIG_DIR` wins, then `$XDG_CONFIG_HOME/cratesdotio-mcp`, then
/// `~/.config/cratesdotio-mcp`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CRATES_MCP_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("cratesdotio-mcp");
    }
    home_dir().join(".config").join("cratesdotio-mcp")
}

pub fn token_path() -> PathBuf {
    config_dir().join("token")
}

pub fn session_path() -> PathBuf {
    config_dir().join("session")
}

pub fn token_meta_path(config_dir: &Path) -> PathBuf {
    config_dir.join("token-meta.json")
}

pub fn audit_path(config_dir: &Path) -> PathBuf {
    config_dir.join("audit.log")
}

pub fn cargo_home() -> PathBuf {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".cargo"))
}

pub fn read_secret(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }

    let value =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value.to_string()))
    }
}

/// Write a secret atomically with owner-only permissions.
///
/// The value goes to a temporary file in the same directory, is synced, then
/// renamed over the target. A crash cannot leave a partial secret behind, and
/// the target inode is created with mode 0600 before any data is written.
pub fn write_secret(path: &Path, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        bail!("refusing to store an empty secret");
    }

    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    // A unique temporary file per writer. A shared name would let concurrent
    // writers unlink or rename each other's unfinished files.
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating a temporary file in {}", parent.display()))?;
    restrict_permissions(file.path())?;

    file.write_all(value.as_bytes())
        .with_context(|| format!("writing a temporary file in {}", parent.display()))?;
    file.as_file()
        .sync_all()
        .with_context(|| format!("syncing a temporary file in {}", parent.display()))?;

    file.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replacing {}", path.display()))?;
    restrict_permissions(path)?;

    #[cfg(unix)]
    if let Ok(directory) = std::fs::File::open(parent) {
        let _ = directory.sync_all();
    }

    Ok(())
}

pub fn remove_secret(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    Ok(true)
}

pub fn stored_token(config_dir: &Path) -> Result<Option<String>> {
    read_secret(&config_dir.join("token"))
}

pub fn stored_cookie(config_dir: &Path) -> Result<Option<String>> {
    read_secret(&config_dir.join("session"))
}

/// What this tool knows about the token it last created.
///
/// crates.io never returns a token value twice, so this record is the only way
/// to show the scopes of the stored token later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMeta {
    pub id: i32,
    pub name: String,
    #[serde(default)]
    pub endpoint_scopes: Vec<String>,
    #[serde(default)]
    pub crate_scopes: Vec<String>,
    pub created_at: String,
    /// First 16 hex characters of SHA-256 of the token value. Used to make
    /// sure the metadata describes the token that is actually loaded.
    #[serde(default)]
    pub token_fingerprint: Option<String>,
}

impl TokenMeta {
    pub fn for_token(
        token: &str,
        id: i32,
        name: String,
        endpoint_scopes: Vec<String>,
        crate_scopes: Vec<String>,
        created_at: String,
    ) -> Self {
        Self {
            id,
            name,
            endpoint_scopes,
            crate_scopes,
            created_at,
            token_fingerprint: Some(token_fingerprint(token)),
        }
    }

    pub fn matches_token(&self, token: &str) -> bool {
        match &self.token_fingerprint {
            Some(stored) => stored == &token_fingerprint(token),
            None => false,
        }
    }

    pub fn scope_summary(&self) -> String {
        let endpoints = if self.endpoint_scopes.is_empty() {
            "all (legacy)".to_string()
        } else {
            self.endpoint_scopes.join(", ")
        };
        let crates = if self.crate_scopes.is_empty() {
            "all crates".to_string()
        } else {
            self.crate_scopes.join(", ")
        };
        format!("{endpoints}; crates: {crates}")
    }
}

fn token_fingerprint(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn write_token_meta(config_dir: &Path, meta: &TokenMeta) -> Result<()> {
    let json = serde_json::to_string_pretty(meta).context("encoding token metadata")?;
    write_secret(&token_meta_path(config_dir), &json)
}

pub fn read_token_meta(config_dir: &Path) -> Result<Option<TokenMeta>> {
    let Some(json) = read_secret(&token_meta_path(config_dir))? else {
        return Ok(None);
    };
    let meta = serde_json::from_str(&json).context("parsing token metadata")?;
    Ok(Some(meta))
}

pub fn remove_token_meta(config_dir: &Path) -> Result<bool> {
    remove_secret(&token_meta_path(config_dir))
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .with_context(|| format!("reading permissions for {}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)
        .with_context(|| format!("restricting permissions on {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");

        write_secret(&path, "  secret-value \n").unwrap();
        assert_eq!(read_secret(&path).unwrap().unwrap(), "secret-value");
    }

    #[test]
    fn refuses_to_write_an_empty_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");

        assert!(write_secret(&path, "   ").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn removes_secrets_and_reports_whether_a_file_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session");

        assert!(!remove_secret(&path).unwrap());
        write_secret(&path, "cookie").unwrap();
        assert!(remove_secret(&path).unwrap());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn secrets_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        write_secret(&path, "secret").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn tightens_a_preexisting_loose_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_secret(&path, "new-secret").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new-secret");
    }

    #[test]
    fn token_metadata_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let meta = TokenMeta::for_token(
            "secret-token",
            7,
            "laptop".to_string(),
            vec!["publish-update".to_string(), "yank".to_string()],
            vec!["demo-*".to_string()],
            "2026-11-08T00:00:00Z".to_string(),
        );

        write_token_meta(dir.path(), &meta).unwrap();
        let loaded = read_token_meta(dir.path()).unwrap().unwrap();

        assert_eq!(loaded.id, 7);
        assert_eq!(loaded.endpoint_scopes, vec!["publish-update", "yank"]);
        assert_eq!(loaded.crate_scopes, vec!["demo-*"]);
        assert!(loaded.matches_token("secret-token"));
    }

    #[test]
    fn token_metadata_matches_only_its_token() {
        let meta = TokenMeta::for_token(
            "token-a",
            7,
            "laptop".to_string(),
            vec!["yank".to_string()],
            Vec::new(),
            "2026-11-08T00:00:00Z".to_string(),
        );

        assert!(meta.matches_token("token-a"));
        assert!(!meta.matches_token("token-b"));
    }

    #[test]
    fn concurrent_writes_never_leave_a_partial_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path_a = dir.path().join("token");
        let path_b = path_a.clone();

        let first = std::thread::spawn(move || write_secret(&path_a, &"a".repeat(4096)));
        let second = std::thread::spawn(move || write_secret(&path_b, &"b".repeat(4096)));
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        let content = std::fs::read_to_string(dir.path().join("token")).unwrap();
        assert!(
            content == "a".repeat(4096) || content == "b".repeat(4096),
            "unexpected partial content of {} bytes",
            content.len()
        );
    }

    #[test]
    fn malformed_token_metadata_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_secret(&token_meta_path(dir.path()), "not json").unwrap();

        assert!(read_token_meta(dir.path()).is_err());
    }
}
