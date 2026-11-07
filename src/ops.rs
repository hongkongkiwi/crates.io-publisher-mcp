use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub const MAX_NAME_LENGTH: usize = 64;
pub const MAX_VERSION_LENGTH: usize = 150;
pub const MAX_DESCRIPTION_LENGTH: usize = 1000;

/// Validate a crate name using the same rules as crates.io.
///
/// Non-empty, at most 64 characters, first character ASCII alphabetic, remaining
/// characters ASCII alphanumeric, `-` or `_`.
pub fn validate_crate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("crate name cannot be empty");
    }
    if name.chars().count() > MAX_NAME_LENGTH {
        bail!("crate name is too long (max {MAX_NAME_LENGTH} characters)");
    }

    let mut chars = name.chars();
    let first = chars.next().expect("name is not empty");
    if first.is_ascii_digit() {
        bail!("crate name cannot start with a digit");
    }
    if !first.is_ascii_alphabetic() {
        bail!("crate name must start with an ASCII letter");
    }
    for ch in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            bail!("invalid character `{ch}` in crate name; use ASCII letters, digits, `-` or `_`");
        }
    }

    Ok(())
}

/// Validate a version before it is placed in a URL path.
pub fn validate_version(version: &str) -> Result<()> {
    if version.is_empty() {
        bail!("version cannot be empty");
    }
    if version.chars().count() > MAX_VERSION_LENGTH {
        bail!("version is too long (max {MAX_VERSION_LENGTH} characters)");
    }
    if !version.starts_with(|ch: char| ch.is_ascii_digit()) {
        bail!("version must start with a digit");
    }
    if !version
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+'))
    {
        bail!("version `{version}` contains characters that are not valid in a semver version");
    }
    Ok(())
}

fn validate_description(description: &str) -> Result<()> {
    if description.trim().is_empty() {
        bail!("description cannot be empty; crates.io requires one");
    }
    if description.chars().count() > MAX_DESCRIPTION_LENGTH {
        bail!("description is too long (max {MAX_DESCRIPTION_LENGTH} characters)");
    }
    Ok(())
}

fn validate_keywords(keywords: &[String]) -> Result<()> {
    if keywords.len() > 5 {
        bail!("at most 5 keywords are allowed");
    }
    for keyword in keywords {
        if keyword.is_empty() || keyword.chars().count() > 20 {
            bail!("keyword `{keyword}` must be between 1 and 20 characters");
        }
        if !keyword
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            bail!("keyword `{keyword}` may only contain ASCII letters, digits, `-` or `_`");
        }
    }
    Ok(())
}

fn validate_categories(categories: &[String]) -> Result<()> {
    if categories.len() > 5 {
        bail!("at most 5 categories are allowed");
    }
    for category in categories {
        if category.is_empty()
            || !category
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | ':'))
        {
            bail!(
                "category `{category}` is not a crates.io category slug; use lowercase names such as `command-line-utilities` or `development-tools::cargo-plugins`"
            );
        }
    }
    Ok(())
}

fn validate_optional_text(label: &str, value: Option<&str>) -> Result<()> {
    if let Some(value) = value
        && value.trim().is_empty()
    {
        bail!("{label} cannot be empty when provided");
    }
    Ok(())
}

fn validate_repository(repository: Option<&str>) -> Result<()> {
    if let Some(repository) = repository
        && !(repository.starts_with("https://") || repository.starts_with("http://"))
    {
        bail!("repository must be an http:// or https:// URL");
    }
    Ok(())
}

pub fn validate_workflow_filename(filename: &str) -> Result<()> {
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') {
        bail!("workflow filename must be a bare file name, not a path");
    }
    if !(filename.ends_with(".yml") || filename.ends_with(".yaml")) {
        bail!("workflow filename must end with .yml or .yaml");
    }
    if filename.chars().count() > 255 {
        bail!("workflow filename is too long");
    }
    Ok(())
}

fn validate_tag_pattern(pattern: &str) -> Result<()> {
    if pattern.is_empty()
        || !pattern
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '/' | '*'))
    {
        bail!("tag pattern may only contain ASCII letters, digits, `.`, `-`, `_`, `/` or `*`");
    }
    Ok(())
}

pub fn validate_environment(environment: &str) -> Result<()> {
    if environment.is_empty()
        || !environment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        bail!("environment may only contain ASCII letters, digits, `.`, `-` or `_`");
    }
    Ok(())
}

pub fn validate_github_name(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("{label} `{value}` is not a valid GitHub name");
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ScaffoldInput {
    pub name: String,
    pub parent_dir: PathBuf,
    pub kind: String,
    pub description: String,
    pub license: Option<String>,
    pub repository: Option<String>,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub edition: String,
}

#[derive(Debug, Clone)]
pub struct ScaffoldOutcome {
    pub directory: PathBuf,
    pub manifest_path: PathBuf,
    pub readme_path: PathBuf,
}

#[derive(Serialize)]
struct ScaffoldManifest<'a> {
    package: ScaffoldPackage<'a>,
    dependencies: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct ScaffoldPackage<'a> {
    name: &'a str,
    version: &'a str,
    edition: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<&'a str>,
    readme: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    keywords: Vec<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    categories: Vec<&'a str>,
}

pub fn render_manifest(input: &ScaffoldInput) -> Result<String> {
    validate_crate_name(&input.name)?;
    validate_description(&input.description)?;
    validate_keywords(&input.keywords)?;
    validate_categories(&input.categories)?;
    validate_optional_text("license", input.license.as_deref())?;
    validate_repository(input.repository.as_deref())?;
    if input.edition.trim().is_empty() {
        bail!("edition cannot be empty");
    }

    let manifest = ScaffoldManifest {
        package: ScaffoldPackage {
            name: &input.name,
            version: "0.1.0",
            edition: &input.edition,
            description: &input.description,
            license: input.license.as_deref(),
            repository: input.repository.as_deref(),
            readme: "README.md",
            keywords: input.keywords.iter().map(String::as_str).collect(),
            categories: input.categories.iter().map(String::as_str).collect(),
        },
        dependencies: BTreeMap::new(),
    };

    toml::to_string_pretty(&manifest).context("rendering Cargo.toml")
}

fn render_readme(input: &ScaffoldInput) -> String {
    let mut readme = format!("# {}\n\n{}\n", input.name, input.description);
    if let Some(repository) = &input.repository {
        readme.push_str(&format!("\nRepository: {repository}\n"));
    }
    if let Some(license) = &input.license {
        readme.push_str(&format!("\n## License\n\n{license}\n"));
    }
    readme
}

#[derive(Debug, Clone)]
pub struct PublishInput {
    pub directory: PathBuf,
    pub dry_run: bool,
    pub confirm: Option<String>,
    pub allow_dirty: bool,
    pub no_verify: bool,
}

#[derive(Debug)]
pub struct PublishOutcome {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub name: String,
    pub version: String,
    pub dry_run: bool,
    pub warnings: Vec<String>,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

#[derive(Debug, Clone)]
pub struct WorkflowInput {
    pub directory: PathBuf,
    pub crate_name: String,
    pub filename: String,
    pub tag_pattern: String,
    pub environment: Option<String>,
    pub run_tests: bool,
    pub locked: bool,
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct WorkflowOutcome {
    pub path: PathBuf,
    pub content: String,
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct ReadManifest {
    package: ReadPackage,
}

#[derive(Deserialize)]
struct ReadPackage {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default, rename = "license-file")]
    license_file: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    publish: Option<PublishField>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PublishField {
    Allowed(bool),
    Registries(Vec<String>),
}

#[derive(Debug)]
struct PublishPreflight {
    name: String,
    version: String,
    warnings: Vec<String>,
}

/// Reject workspace-inherited values with a clear message instead of a serde
/// type error. Resolving them would duplicate cargo's workspace rules.
fn reject_workspace_inheritance(manifest: &toml::Value) -> Result<()> {
    const INHERITABLE: [&str; 6] = [
        "version",
        "description",
        "license",
        "license-file",
        "repository",
        "publish",
    ];

    let Some(package) = manifest.get("package").and_then(toml::Value::as_table) else {
        return Ok(());
    };

    for field in INHERITABLE {
        let inherited = package
            .get(field)
            .and_then(toml::Value::as_table)
            .is_some_and(|table| table.contains_key("workspace"));
        if inherited {
            bail!(
                "Cargo.toml inherits `{field}` from [workspace.package]. This tool needs literal values so it can confirm the exact name and version before publishing. Run `cargo publish` directly for this crate, or copy the inherited values into the manifest."
            );
        }
    }

    Ok(())
}

fn publish_preflight(
    manifest_path: &Path,
    confirm: Option<&str>,
    dry_run: bool,
) -> Result<PublishPreflight> {
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let raw: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;
    reject_workspace_inheritance(&raw)?;
    let manifest: ReadManifest =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;
    let package = manifest.package;

    validate_crate_name(&package.name)?;
    validate_version(&package.version)?;

    match &package.publish {
        None => {}
        Some(PublishField::Allowed(false)) => {
            bail!("Cargo.toml sets `publish = false`, so this crate cannot be published");
        }
        Some(PublishField::Allowed(true)) => {}
        Some(PublishField::Registries(registries)) => {
            if !registries.iter().any(|registry| registry == "crates-io") {
                bail!(
                    "Cargo.toml restricts publishing to {registries:?}, which does not include `crates-io`"
                );
            }
        }
    }

    if package
        .description
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        bail!("Cargo.toml is missing a `description`; crates.io rejects publishes without one");
    }

    let mut warnings = Vec::new();
    if package.license.is_none() && package.license_file.is_none() {
        warnings.push(
            "Cargo.toml has no `license` or `license-file`; crates.io accepts this but it is strongly discouraged".to_string(),
        );
    }
    if package.repository.is_none() {
        warnings.push(
            "Cargo.toml has no `repository`; the crates.io page will not link to source"
                .to_string(),
        );
    }

    let expected = format!("{}@{}", package.name, package.version);
    if !dry_run {
        match confirm {
            Some(confirm) if confirm == expected => {}
            Some(confirm) => bail!("confirm must be exactly `{expected}` (received `{confirm}`)"),
            None => bail!("refusing to publish without confirm = \"{expected}\""),
        }
    }

    Ok(PublishPreflight {
        name: package.name,
        version: package.version,
        warnings,
    })
}

pub fn render_workflow(input: &WorkflowInput) -> Result<String> {
    validate_crate_name(&input.crate_name)?;
    validate_workflow_filename(&input.filename)?;
    validate_tag_pattern(&input.tag_pattern)?;
    if let Some(environment) = &input.environment {
        validate_environment(environment)?;
    }

    let locked = if input.locked { " --locked" } else { "" };
    let mut out = String::new();

    out.push_str("name: Publish\n\n");
    out.push_str("on:\n  push:\n    tags:\n");
    out.push_str(&format!("      - \"{}\"\n", input.tag_pattern));
    out.push_str("  workflow_dispatch:\n\n");
    out.push_str("permissions:\n  contents: read\n\n");
    out.push_str("jobs:\n  publish:\n    runs-on: ubuntu-latest\n");
    if let Some(environment) = &input.environment {
        out.push_str(&format!("    environment: \"{environment}\"\n"));
    }
    out.push_str("    permissions:\n      contents: read\n      id-token: write\n    steps:\n");
    out.push_str("      - uses: actions/checkout@v4\n\n");
    out.push_str("      - uses: dtolnay/rust-toolchain@stable\n");
    if input.run_tests {
        out.push_str(&format!(
            "\n      - name: Run tests\n        run: cargo test{locked}\n"
        ));
    }
    out.push_str("\n      - uses: rust-lang/crates-io-auth-action@v1\n        id: auth\n\n");
    out.push_str("      - name: Publish to crates.io\n");
    out.push_str(&format!(
        "        run: cargo publish --registry crates-io{locked}\n"
    ));
    out.push_str("        env:\n");
    out.push_str("          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}\n");
    out.push_str("          CARGO_REGISTRIES_CRATES_IO_TOKEN: ${{ steps.auth.outputs.token }}\n");

    Ok(out)
}

#[derive(Debug)]
struct CommandOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

impl CommandOutput {
    fn success(&self) -> bool {
        self.status.success()
    }

    fn exit_code(&self) -> Option<i32> {
        self.status.code()
    }
}

#[derive(Debug)]
pub struct CargoOps {
    cargo: String,
}

impl Default for CargoOps {
    fn default() -> Self {
        Self {
            cargo: std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()),
        }
    }
}

impl CargoOps {
    async fn run(
        &self,
        args: &[String],
        directory: &Path,
        token: Option<&str>,
        timeout: Duration,
    ) -> Result<CommandOutput> {
        let mut command = Command::new(&self.cargo);
        command
            .args(args)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("CARGO_TERM_COLOR", "never")
            .kill_on_drop(true);

        if let Some(token) = token {
            command
                .env("CARGO_REGISTRY_TOKEN", token)
                .env("CARGO_REGISTRIES_CRATES_IO_TOKEN", token);
        }

        let output = tokio::time::timeout(timeout, command.output())
            .await
            .with_context(|| {
                format!(
                    "cargo timed out after {} seconds; the process was killed",
                    timeout.as_secs()
                )
            })?
            .with_context(|| {
                format!(
                    "failed to run `{}`; is cargo installed and on PATH?",
                    self.cargo
                )
            })?;

        Ok(CommandOutput {
            status: output.status,
            stdout: redact_token(&String::from_utf8_lossy(&output.stdout), token),
            stderr: redact_token(&String::from_utf8_lossy(&output.stderr), token),
        })
    }

    pub async fn scaffold(&self, input: &ScaffoldInput) -> Result<ScaffoldOutcome> {
        validate_crate_name(&input.name)?;
        if !input.parent_dir.is_absolute() {
            bail!("parent_dir must be an absolute path");
        }
        if !input.parent_dir.is_dir() {
            bail!(
                "parent_dir {} does not exist or is not a directory",
                input.parent_dir.display()
            );
        }

        let flag = match input.kind.as_str() {
            "lib" => "--lib",
            "bin" => "--bin",
            other => bail!("kind must be `lib` or `bin`, not `{other}`"),
        };

        // Render first so manifest validation fails before anything touches disk.
        let manifest = render_manifest(input)?;

        let directory = input.parent_dir.join(&input.name);
        if directory.exists() {
            bail!(
                "{} already exists; refusing to overwrite it",
                directory.display()
            );
        }

        let args = vec![
            "new".to_string(),
            flag.to_string(),
            "--name".to_string(),
            input.name.clone(),
            directory.display().to_string(),
        ];
        let output = self
            .run(&args, &input.parent_dir, None, Duration::from_secs(120))
            .await?;
        if !output.success() {
            bail!("cargo new failed: {}", tail(&output.stderr, 20));
        }

        let manifest_path = directory.join("Cargo.toml");
        std::fs::write(&manifest_path, manifest)
            .with_context(|| format!("writing {}", manifest_path.display()))?;

        let readme_path = directory.join("README.md");
        std::fs::write(&readme_path, render_readme(input))
            .with_context(|| format!("writing {}", readme_path.display()))?;

        Ok(ScaffoldOutcome {
            directory,
            manifest_path,
            readme_path,
        })
    }

    pub async fn publish(&self, input: &PublishInput, token: &str) -> Result<PublishOutcome> {
        if !input.directory.is_absolute() {
            bail!("directory must be an absolute path");
        }
        let directory = std::fs::canonicalize(&input.directory)
            .with_context(|| format!("resolving crate directory {}", input.directory.display()))?;
        let manifest_path = directory.join("Cargo.toml");
        if !manifest_path.is_file() {
            bail!("{} does not contain a Cargo.toml", directory.display());
        }

        let preflight = publish_preflight(&manifest_path, input.confirm.as_deref(), input.dry_run)?;

        // Force crates.io even when the user's cargo config names a different
        // default registry. Both token env vars are set for the child process.
        let mut args = vec![
            "publish".to_string(),
            "--registry".to_string(),
            "crates-io".to_string(),
        ];
        if input.dry_run {
            args.push("--dry-run".to_string());
        }
        if input.allow_dirty {
            args.push("--allow-dirty".to_string());
        }
        if input.no_verify {
            args.push("--no-verify".to_string());
        }
        if directory.join("Cargo.lock").is_file() {
            args.push("--locked".to_string());
        }

        let timeout = if input.dry_run {
            Duration::from_secs(600)
        } else {
            Duration::from_secs(1200)
        };
        let output = self.run(&args, &directory, Some(token), timeout).await?;

        Ok(PublishOutcome {
            success: output.success(),
            exit_code: output.exit_code(),
            name: preflight.name,
            version: preflight.version,
            dry_run: input.dry_run,
            warnings: preflight.warnings,
            stdout_tail: tail(&output.stdout, 40),
            stderr_tail: tail(&output.stderr, 40),
        })
    }

    pub fn write_workflow(&self, input: &WorkflowInput) -> Result<WorkflowOutcome> {
        if !input.directory.is_absolute() {
            bail!("directory must be an absolute path");
        }
        let directory = std::fs::canonicalize(&input.directory)
            .with_context(|| format!("resolving crate directory {}", input.directory.display()))?;
        if !directory.join("Cargo.toml").is_file() {
            bail!("{} does not contain a Cargo.toml", directory.display());
        }

        let content = render_workflow(input)?;
        let workflows = directory.join(".github").join("workflows");
        let path = workflows.join(&input.filename);
        if path.exists() && !input.overwrite {
            bail!(
                "{} already exists; pass overwrite = true to replace it",
                path.display()
            );
        }
        std::fs::create_dir_all(&workflows)
            .with_context(|| format!("creating {}", workflows.display()))?;
        std::fs::write(&path, &content).with_context(|| format!("writing {}", path.display()))?;

        let mut warnings = Vec::new();
        if !input.run_tests {
            warnings.push(
                "the workflow does not run tests before publishing; consider run_tests = true"
                    .to_string(),
            );
        }
        if !input.locked {
            warnings.push(
                "the workflow does not pass --locked; commit Cargo.lock and set locked = true for reproducible publishes"
                    .to_string(),
            );
        }

        Ok(WorkflowOutcome {
            path,
            content,
            warnings,
        })
    }
}

fn redact_token(value: &str, explicit: Option<&str>) -> String {
    // Scaffolding runs without passing a token, but the child inherits the
    // parent environment, so scrub any token-shaped environment value too.
    let env_secrets = [
        std::env::var("CRATES_IO_TOKEN").ok(),
        std::env::var("CARGO_REGISTRY_TOKEN").ok(),
        std::env::var("CARGO_REGISTRIES_CRATES_IO_TOKEN").ok(),
    ];
    let mut secrets: Vec<&str> = env_secrets.iter().flatten().map(String::as_str).collect();
    if let Some(secret) = explicit {
        secrets.push(secret);
    }

    let mut redacted = value.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            redacted = redacted.replace(secret, "<redacted-token>");
        }
    }
    redacted
}

fn tail(value: &str, lines: usize) -> String {
    let collected: Vec<&str> = value.lines().collect();
    let start = collected.len().saturating_sub(lines);
    let tail = collected[start..].join("\n");
    if tail.chars().count() > 8000 {
        format!("{}...", tail.chars().take(8000).collect::<String>())
    } else {
        tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NewGithubConfig;

    fn scaffold_input() -> ScaffoldInput {
        ScaffoldInput {
            name: "example-crate".to_string(),
            parent_dir: PathBuf::from("/tmp"),
            kind: "lib".to_string(),
            description: "An example crate".to_string(),
            license: Some("MIT".to_string()),
            repository: Some("https://github.com/example/example-crate".to_string()),
            keywords: vec!["testing".to_string()],
            categories: vec!["development-tools".to_string()],
            edition: "2024".to_string(),
        }
    }

    fn workflow_input() -> WorkflowInput {
        WorkflowInput {
            directory: PathBuf::from("/tmp/example-crate"),
            crate_name: "example-crate".to_string(),
            filename: "publish.yml".to_string(),
            tag_pattern: "v*".to_string(),
            environment: None,
            run_tests: true,
            locked: false,
            overwrite: false,
        }
    }

    #[test]
    fn accepts_names_crates_io_accepts() {
        for name in ["serde", "tokio-util", "foo_bar", "a1"] {
            validate_crate_name(name).unwrap();
        }
    }

    #[test]
    fn rejects_names_crates_io_rejects() {
        for name in ["", "1foo", "_foo", "-foo", "foo bar", "foo.bar", "föö"] {
            assert!(
                validate_crate_name(name).is_err(),
                "expected `{name}` to fail"
            );
        }
        assert!(validate_crate_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn validates_versions_that_are_safe_in_urls() {
        validate_version("1.2.3").unwrap();
        validate_version("1.2.3-alpha.1+build.7").unwrap();
        assert!(validate_version("").is_err());
        assert!(validate_version(".").is_err());
        assert!(validate_version("..").is_err());
        assert!(validate_version("v1.2.3").is_err());
        assert!(validate_version("1.2.3/../x").is_err());
        assert!(validate_version("1.2.3 beta").is_err());
    }

    #[test]
    fn renders_a_publishable_manifest() {
        let manifest = render_manifest(&scaffold_input()).unwrap();

        assert!(manifest.contains("[package]"));
        assert!(manifest.contains("name = \"example-crate\""));
        assert!(manifest.contains("description = \"An example crate\""));
        assert!(manifest.contains("license = \"MIT\""));
        assert!(manifest.contains("readme = \"README.md\""));
        assert!(manifest.contains("keywords = [\"testing\"]"));
        assert!(manifest.contains("[dependencies]"));
    }

    #[test]
    fn rejects_a_manifest_without_a_description() {
        let mut input = scaffold_input();
        input.description = "   ".to_string();

        assert!(render_manifest(&input).is_err());
    }

    #[test]
    fn rejects_too_many_keywords() {
        let mut input = scaffold_input();
        input.keywords = (0..6).map(|index| format!("k{index}")).collect();

        assert!(render_manifest(&input).is_err());
    }

    #[test]
    fn redacts_the_token_from_subprocess_output() {
        assert_eq!(
            redact_token("warning: CARGO_REGISTRY_TOKEN=secret", Some("secret")),
            "warning: CARGO_REGISTRY_TOKEN=<redacted-token>"
        );
        assert_eq!(redact_token("no token here", None), "no token here");
        assert_eq!(redact_token("no token here", Some("")), "no token here");
    }

    #[test]
    fn rejects_workspace_inherited_metadata_with_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        std::fs::write(
            &manifest_path,
            "[package]\nname = \"example\"\nversion.workspace = true\n",
        )
        .unwrap();

        let error = publish_preflight(&manifest_path, None, true)
            .unwrap_err()
            .to_string();

        assert!(error.contains("workspace.package"), "{error}");
        assert!(error.contains("version"), "{error}");
    }

    #[test]
    fn recognizes_the_hyphenated_license_file_key() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        std::fs::write(
            &manifest_path,
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\ndescription = \"x\"\nlicense-file = \"LICENSE\"\n",
        )
        .unwrap();

        let preflight = publish_preflight(&manifest_path, None, true).unwrap();

        assert!(
            !preflight
                .warnings
                .iter()
                .any(|warning| warning.contains("license")),
            "{:?}",
            preflight.warnings
        );
    }

    #[test]
    fn renders_a_trusted_publishing_workflow() {
        let workflow = render_workflow(&workflow_input()).unwrap();

        assert!(workflow.contains("id-token: write"));
        assert!(workflow.contains("rust-lang/crates-io-auth-action@v1"));
        assert!(workflow.contains("CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}"));
        assert!(
            workflow.contains("CARGO_REGISTRIES_CRATES_IO_TOKEN: ${{ steps.auth.outputs.token }}")
        );
        assert!(workflow.contains("- \"v*\""));
        assert!(workflow.contains("cargo test"));
        assert!(workflow.contains("cargo publish --registry crates-io"));
        assert!(!workflow.contains("--locked"));
    }

    #[test]
    fn workflow_can_lock_and_use_an_environment() {
        let mut input = workflow_input();
        input.locked = true;
        input.environment = Some("release".to_string());
        let workflow = render_workflow(&input).unwrap();

        assert!(workflow.contains("environment: \"release\""));
        assert!(workflow.contains("cargo test --locked"));
        assert!(workflow.contains("cargo publish --registry crates-io --locked"));
    }

    #[test]
    fn rejects_workflow_filenames_that_escape_the_directory() {
        for filename in ["../publish.yml", "dir/publish.yml", "publish.txt", ""] {
            assert!(
                validate_workflow_filename(filename).is_err(),
                "expected `{filename}` to fail"
            );
        }
    }

    #[test]
    fn tails_only_the_requested_number_of_lines() {
        assert_eq!(tail("a\nb\nc", 2), "b\nc");
        assert_eq!(tail("a", 5), "a");
    }

    #[test]
    fn trustpub_config_serializes_crates_io_field_names() {
        let config = NewGithubConfig {
            krate: "example-crate",
            repository_owner: "example",
            repository_name: "example-crate",
            workflow_filename: "publish.yml",
            environment: None,
        };
        let value = serde_json::to_value(&config).unwrap();

        assert_eq!(value["krate"], "example-crate");
        assert_eq!(value["workflow_filename"], "publish.yml");
        assert!(value.get("environment").is_none());
    }
}
