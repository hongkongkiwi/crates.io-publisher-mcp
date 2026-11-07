use super::schema_for;
use crate::audit;
use crate::auth::Token;
use crate::credentials;
use crate::ops::{
    CargoOps, PublishInput, PublishOutcome, ScaffoldInput, ScaffoldOutcome, WorkflowInput,
    WorkflowOutcome,
};
use anyhow::{Result, anyhow};
use rmcp::model::{CallToolResult, Content, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::mode::Mode;

/// Tools that create a crate locally, publish it, or write CI workflows.
pub struct PublishTools {
    ops: Arc<CargoOps>,
    token: Token,
    mode: Mode,
}

impl PublishTools {
    pub const SCAFFOLD: &'static str = "scaffold_crate";
    pub const PUBLISH: &'static str = "publish_crate";
    pub const WORKFLOW: &'static str = "generate_publish_workflow";

    pub fn new(ops: Arc<CargoOps>, token: Token, mode: Mode) -> Self {
        Self { ops, token, mode }
    }

    pub fn tools(&self) -> Vec<Tool> {
        vec![
            Tool::new(
                Self::SCAFFOLD,
                "Create a new Rust crate directory with a publishable Cargo.toml and README, and check that the name is valid before touching disk. This does not contact crates.io and does not publish anything.",
                schema_for::<ScaffoldCrateInput>(),
            ),
            Tool::new(
                Self::PUBLISH,
                "Run `cargo publish` in an existing crate directory. Defaults to `dry_run = true`, which packages and verifies the crate without uploading. A real publish requires `dry_run = false` and `confirm` set to `name@version` exactly. Publishing is irreversible after 72 hours and a version number can never be reused.",
                schema_for::<PublishCrateInput>(),
            ),
            Tool::new(
                Self::WORKFLOW,
                "Write a GitHub Actions workflow that publishes with crates.io trusted publishing (OIDC), so CI needs no long-lived token. The workflow file name must match the trusted publishing config registered on crates.io.",
                schema_for::<GenerateWorkflowInput>(),
            ),
        ]
    }

    pub async fn execute(
        &self,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<Option<CallToolResult>> {
        let result = match name {
            Self::SCAFFOLD => self.scaffold(deserialize(args)?).await?,
            Self::PUBLISH => self.publish(deserialize(args)?).await?,
            Self::WORKFLOW => self.workflow(deserialize(args)?)?,
            _ => return Ok(None),
        };

        Ok(Some(result))
    }

    async fn scaffold(&self, input: ScaffoldCrateInput) -> Result<CallToolResult> {
        let scaffold = ScaffoldInput {
            name: input.name,
            parent_dir: PathBuf::from(input.parent_dir),
            kind: input.kind,
            description: input.description,
            license: input.license,
            repository: input.repository,
            keywords: input.keywords,
            categories: input.categories,
            edition: input.edition,
        };

        match self.ops.scaffold(&scaffold).await {
            Ok(outcome) => Ok(success_result(format_scaffold(&outcome))),
            Err(error) => Ok(error_result(format!("Failed to scaffold crate: {error}"))),
        }
    }

    async fn publish(&self, input: PublishCrateInput) -> Result<CallToolResult> {
        let publish = PublishInput {
            directory: PathBuf::from(input.directory),
            dry_run: input.dry_run,
            confirm: input.confirm,
            allow_dirty: input.allow_dirty,
            no_verify: input.no_verify,
        };

        let result = self.ops.publish(&publish, self.token.expose()).await;
        match &result {
            Ok(outcome) => {
                let detail = outcome.exit_code.map(|code| code.to_string());
                audit::record_or_warn(
                    &credentials::config_dir(),
                    self.mode.as_str(),
                    if outcome.dry_run {
                        "publish-dry-run"
                    } else {
                        "publish"
                    },
                    &format!("{}@{}", outcome.name, outcome.version),
                    if outcome.success { "ok" } else { "error" },
                    detail.as_deref(),
                );
            }
            Err(_) => {
                audit_publish_error(&credentials::config_dir(), self.mode, publish.dry_run);
            }
        }

        match result {
            Ok(outcome) if outcome.success => Ok(success_result(format_publish(&outcome))),
            Ok(outcome) => Ok(error_result(format_publish(&outcome))),
            Err(error) => {
                let mut message = error.to_string();
                if let Some(confirm) = publish.confirm.as_deref().filter(|value| !value.is_empty())
                {
                    message = message.replace(confirm, "[redacted]");
                }
                Ok(error_result(format!("Publish failed: {message}")))
            }
        }
    }

    fn workflow(&self, input: GenerateWorkflowInput) -> Result<CallToolResult> {
        let workflow = WorkflowInput {
            directory: PathBuf::from(input.directory),
            crate_name: input.name,
            filename: input.filename,
            tag_pattern: input.tag_pattern,
            environment: input.environment,
            run_tests: input.run_tests,
            locked: input.locked,
            pin_actions: input.pin_actions,
            overwrite: input.overwrite,
        };

        match self.ops.write_workflow(&workflow) {
            Ok(outcome) => Ok(success_result(format_workflow(&outcome))),
            Err(error) => Ok(error_result(format!("Failed to write workflow: {error}"))),
        }
    }
}

fn deserialize<T: DeserializeOwned>(args: Map<String, Value>) -> Result<T> {
    serde_json::from_value(Value::Object(args))
        .map_err(|error| anyhow!("invalid parameters: {error}"))
}

fn success_result(text: String) -> CallToolResult {
    CallToolResult::success(vec![Content::text(text)])
}

fn error_result(text: String) -> CallToolResult {
    CallToolResult::error(vec![Content::text(text)])
}

fn audit_publish_error(config_dir: &std::path::Path, mode: Mode, dry_run: bool) {
    audit::record_or_warn(
        config_dir,
        mode.as_str(),
        if dry_run {
            "publish-dry-run"
        } else {
            "publish"
        },
        "unknown",
        "error",
        Some("publish-failed"),
    );
}

fn format_scaffold(outcome: &ScaffoldOutcome) -> String {
    format!(
        "Created {}\n  {}\n  {}\n\nNext steps:\n  1. Add dependencies and code.\n  2. Check the name with `check_crate_name`.\n  3. Commit the new files. cargo refuses to publish a dirty git tree unless `allow_dirty` is true.\n  4. Write a release workflow with `generate_publish_workflow`.\n  5. Package and verify with `publish_crate` and `dry_run: true`.\n  6. Publish with `publish_crate`, `dry_run: false`, and `confirm` set to the name and version.",
        outcome.directory.display(),
        outcome.manifest_path.display(),
        outcome.readme_path.display()
    )
}

fn format_publish(outcome: &PublishOutcome) -> String {
    let mut out = String::new();

    if outcome.success {
        if outcome.dry_run {
            out.push_str(&format!(
                "Dry run succeeded for {}@{}.\n",
                outcome.name, outcome.version
            ));
        } else {
            out.push_str(&format!(
                "Published {}@{}.\n",
                outcome.name, outcome.version
            ));
        }
    } else {
        out.push_str(&format!(
            "cargo publish failed for {}@{} (exit code {}).\n",
            outcome.name,
            outcome.version,
            outcome
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string())
        ));
    }

    for warning in &outcome.warnings {
        out.push_str(&format!("warning: {warning}\n"));
    }

    if !outcome.stderr_tail.is_empty() {
        out.push_str("\nstderr (tail):\n");
        out.push_str(&outcome.stderr_tail);
        out.push('\n');
    }
    if !outcome.stdout_tail.is_empty() {
        out.push_str("\nstdout (tail):\n");
        out.push_str(&outcome.stdout_tail);
        out.push('\n');
    }

    out.trim_end().to_string()
}

fn format_workflow(outcome: &WorkflowOutcome) -> String {
    let mut out = format!(
        "Wrote {}.\n\nRegister this exact file name when creating the trusted publisher:\n  create_trustpub_github_config {{ workflow_filename: \"{}\" }}\n",
        outcome.path.display(),
        outcome
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    );

    for warning in &outcome.warnings {
        out.push_str(&format!("warning: {warning}\n"));
    }

    out.push_str("\n--- workflow ---\n");
    out.push_str(&outcome.content);
    out
}

fn default_kind() -> String {
    "lib".to_string()
}

fn default_edition() -> String {
    "2024".to_string()
}

fn default_true() -> bool {
    true
}

fn default_workflow_filename() -> String {
    "publish.yml".to_string()
}

fn default_tag_pattern() -> String {
    "v*".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScaffoldCrateInput {
    /// Crate name. Must be a valid crates.io name: ASCII letter first, then letters, digits, `-` or `_`, at most 64 characters
    pub name: String,
    /// Absolute path to the directory that will contain the crate directory
    pub parent_dir: String,
    /// Crate kind: `lib` or `bin`
    #[serde(default = "default_kind")]
    pub kind: String,
    /// One-line description. crates.io requires this for publishing
    pub description: String,
    /// SPDX license expression, for example `MIT` or `MIT OR Apache-2.0`
    pub license: Option<String>,
    /// Source repository URL
    pub repository: Option<String>,
    /// Up to 5 keywords, each at most 20 characters
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Up to 5 crates.io category slugs, for example `command-line-utilities`
    #[serde(default)]
    pub categories: Vec<String>,
    /// Rust edition
    #[serde(default = "default_edition")]
    pub edition: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PublishCrateInput {
    /// Absolute path to the crate directory (the directory containing Cargo.toml)
    pub directory: String,
    /// When true (the default), run `cargo publish --dry-run` and do not upload
    #[serde(default = "default_true")]
    pub dry_run: bool,
    /// Required for a real publish. Must be exactly `name@version` as written in Cargo.toml
    pub confirm: Option<String>,
    /// Pass `--allow-dirty` to cargo
    #[serde(default)]
    pub allow_dirty: bool,
    /// Pass `--no-verify` to cargo. This skips the verification build and is not recommended
    #[serde(default)]
    pub no_verify: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::TokenSource;

    #[tokio::test]
    async fn publish_error_audit_excludes_request_values() {
        if let Some(dir) = std::env::var_os("PUBLISH_AUDIT_TEST_DIR") {
            let secret = "secret-request-value";
            let tools = PublishTools::new(
                Arc::new(CargoOps::default()),
                Token::new("token".into(), TokenSource::CratesIoTokenEnv, None),
                Mode::Publish,
            );
            tools
                .publish(PublishCrateInput {
                    directory: PathBuf::from(&dir)
                        .join(secret)
                        .to_string_lossy()
                        .into_owned(),
                    dry_run: false,
                    confirm: Some(secret.into()),
                    allow_dirty: false,
                    no_verify: false,
                })
                .await
                .unwrap();
            let log = std::fs::read_to_string(credentials::audit_path(std::path::Path::new(&dir)))
                .unwrap();
            assert!(!log.contains(secret));
            let record: Value = serde_json::from_str(log.trim()).unwrap();
            assert_eq!(record["target"], "unknown");
            assert_eq!(record["detail"], "publish-failed");
            assert_eq!(record["result"], "error");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "server::tools::publish::tests::publish_error_audit_excludes_request_values",
            ])
            .env("PUBLISH_AUDIT_TEST_DIR", dir.path())
            .env("CRATES_MCP_CONFIG_DIR", dir.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GenerateWorkflowInput {
    /// Absolute path to the crate directory (the directory containing Cargo.toml)
    pub directory: String,
    /// Name of the crate. Used for validation
    pub name: String,
    /// Workflow file name, for example `publish.yml`. Must match the trusted publishing config
    #[serde(default = "default_workflow_filename")]
    pub filename: String,
    /// Git tag pattern that triggers a publish
    #[serde(default = "default_tag_pattern")]
    pub tag_pattern: String,
    /// Optional GitHub Actions environment name. It must match the environment in the trusted publishing config
    pub environment: Option<String>,
    /// Run `cargo test` before publishing
    #[serde(default = "default_true")]
    pub run_tests: bool,
    /// Pass `--locked` to cargo. Commit Cargo.lock first
    #[serde(default)]
    pub locked: bool,
    /// Pin actions to commit SHAs instead of moving tags or branches
    #[serde(default = "default_true")]
    pub pin_actions: bool,
    /// Replace an existing workflow file
    #[serde(default)]
    pub overwrite: bool,
}
