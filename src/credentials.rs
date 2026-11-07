use anyhow::{Context, Result};
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

/// Write a secret with owner-only permissions.
pub fn write_secret(path: &Path, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("refusing to store an empty secret");
    }

    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    // Tighten an existing file before writing, so a secret never sits in a
    // world-readable file, even briefly.
    if path.exists() {
        restrict_permissions(path)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("writing {}", path.display()))?;
        file.write_all(value.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(path, value).with_context(|| format!("writing {}", path.display()))?;
    }

    restrict_permissions(path)?;
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
}
