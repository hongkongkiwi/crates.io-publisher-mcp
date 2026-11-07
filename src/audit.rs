use crate::credentials;
use anyhow::{Context, Result};
use serde::Serialize;
use std::io::Write;
use std::path::Path;

#[derive(Serialize)]
struct Record<'a> {
    timestamp: String,
    mode: &'a str,
    action: &'a str,
    target: &'a str,
    result: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

/// Append one audit line to `$CONFIG_DIR/audit.log`.
///
/// Records carry what happened, never credential values. Callers should treat
/// a failure as a warning: the operation itself already happened.
pub fn record(
    config_dir: &Path,
    mode: &str,
    action: &str,
    target: &str,
    result: &str,
    detail: Option<&str>,
) -> Result<()> {
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating {}", config_dir.display()))?;

    let path = credentials::audit_path(config_dir);
    let mut line = serde_json::to_string(&Record {
        timestamp: chrono::Utc::now().to_rfc3339(),
        mode,
        action,
        target,
        result,
        detail,
    })
    .context("encoding audit record")?;

    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?
    };

    #[cfg(not(unix))]
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;

    line.push('\n');
    file.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    restrict_permissions(&path)?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("syncing {}", path.display()))?;
    Ok(())
}

pub fn record_or_warn(
    config_dir: &Path,
    mode: &str,
    action: &str,
    target: &str,
    result: &str,
    detail: Option<&str>,
) {
    if let Err(error) = record(config_dir, mode, action, target, result, detail) {
        tracing::warn!("failed to write the audit log: {error:#}");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_one_json_line_per_record() {
        let dir = tempfile::tempdir().unwrap();

        record(dir.path(), "write", "yank", "demo@1.2.3", "ok", None).unwrap();
        record(
            dir.path(),
            "admin",
            "delete-crate",
            "old-crate",
            "error",
            Some("403"),
        )
        .unwrap();

        let log = std::fs::read_to_string(credentials::audit_path(dir.path())).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["mode"], "write");
        assert_eq!(first["action"], "yank");
        assert_eq!(first["target"], "demo@1.2.3");
        assert_eq!(first["result"], "ok");
        assert!(first["timestamp"].is_string());
        assert!(first.get("detail").is_none());

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["detail"], "403");
    }

    #[test]
    fn concurrent_process_writer() {
        let Some(dir) = std::env::var_os("AUDIT_TEST_DIR") else {
            return;
        };
        let id = std::env::var("AUDIT_TEST_ID").unwrap();
        std::thread::scope(|scope| {
            for thread in 0..4 {
                let dir = &dir;
                let id = &id;
                scope.spawn(move || {
                    for item in 0..30 {
                        let target = format!("{id}-{thread}-{item}-{}", "x".repeat(4096));
                        record(Path::new(dir), "publish", "publish", &target, "ok", None).unwrap();
                    }
                });
            }
        });
    }

    #[test]
    fn concurrent_processes_and_threads_append_complete_records() {
        let dir = tempfile::tempdir().unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut children: Vec<_> = (0..4)
            .map(|id| {
                std::process::Command::new(&executable)
                    .args(["--exact", "audit::tests::concurrent_process_writer"])
                    .env("AUDIT_TEST_DIR", dir.path())
                    .env("AUDIT_TEST_ID", id.to_string())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        let log = std::fs::read_to_string(credentials::audit_path(dir.path())).unwrap();
        assert!(log.ends_with('\n'));
        let targets: std::collections::HashSet<_> = log
            .lines()
            .map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                value["target"].as_str().unwrap().to_owned()
            })
            .collect();
        assert_eq!(log.lines().count(), 480);
        assert_eq!(targets.len(), 480);
    }

    #[cfg(unix)]
    #[test]
    fn audit_log_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), "publish", "publish", "demo@0.1.0", "ok", None).unwrap();

        let mode = std::fs::metadata(credentials::audit_path(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
