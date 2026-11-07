use super::info::CrateNameInput;
use super::schema_for;
use crate::api::{GithubConfig, NewGithubConfig, Owner, WriteClient};
use crate::ops::{
    validate_crate_name, validate_environment, validate_github_name, validate_version,
    validate_workflow_filename,
};
use anyhow::{Result, anyhow};
use rmcp::model::{CallToolResult, Content, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::sync::Arc;

/// Tools that change crates.io state through the authenticated API.
pub struct WriteTools {
    client: Arc<WriteClient>,
}

impl WriteTools {
    pub const LIST_OWNERS: &'static str = "list_crate_owners";
    pub const ADD_OWNER: &'static str = "add_crate_owner";
    pub const REMOVE_OWNER: &'static str = "remove_crate_owner";
    pub const YANK: &'static str = "yank_crate_version";
    pub const UNYANK: &'static str = "unyank_crate_version";
    pub const LIST_TRUSTPUB: &'static str = "list_trustpub_github_configs";
    pub const CREATE_TRUSTPUB: &'static str = "create_trustpub_github_config";
    pub const DELETE_TRUSTPUB: &'static str = "delete_trustpub_github_config";
    pub const SET_TRUSTPUB_ONLY: &'static str = "set_trustpub_only";

    pub fn new(client: Arc<WriteClient>) -> Self {
        Self { client }
    }

    pub fn tools(&self) -> Vec<Tool> {
        vec![
            Tool::new(
                Self::LIST_OWNERS,
                "List the owners of a crate. Owner data is public and does not need a token scope.",
                schema_for::<CrateNameInput>(),
            ),
            Tool::new(
                Self::ADD_OWNER,
                "Add a crates.io user or GitHub team as an owner of a crate. Requires the `change-owners` token scope. Users receive an invitation they must accept.",
                schema_for::<OwnerInput>(),
            ),
            Tool::new(
                Self::REMOVE_OWNER,
                "Remove an owner from a crate. Requires the `change-owners` token scope.",
                schema_for::<OwnerInput>(),
            ),
            Tool::new(
                Self::YANK,
                "Yank a published version. Requires the `yank` token scope. Yanked versions stay downloadable but new dependency resolution avoids them.",
                schema_for::<YankInput>(),
            ),
            Tool::new(
                Self::UNYANK,
                "Undo a yank for a published version. Requires the `yank` token scope.",
                schema_for::<UnyankInput>(),
            ),
            Tool::new(
                Self::LIST_TRUSTPUB,
                "List GitHub Actions trusted publishing configurations for a crate. Requires the `trusted-publishing` token scope.",
                schema_for::<CrateNameInput>(),
            ),
            Tool::new(
                Self::CREATE_TRUSTPUB,
                "Register a GitHub Actions workflow as a trusted publisher for a crate. Requires the `trusted-publishing` token scope, plus a crates.io account with a linked GitHub account and a verified email. Without this, CI must use a long-lived publish token instead.",
                schema_for::<CreateTrustpubInput>(),
            ),
            Tool::new(
                Self::DELETE_TRUSTPUB,
                "Delete a GitHub trusted publishing configuration by its id. Requires the `trusted-publishing` token scope.",
                schema_for::<DeleteTrustpubInput>(),
            ),
            Tool::new(
                Self::SET_TRUSTPUB_ONLY,
                "Restrict a crate so that only trusted publishing tokens can publish new versions. Requires the `trusted-publishing` token scope. Long-lived tokens can still yank and manage owners unless they are revoked.",
                schema_for::<SetTrustpubOnlyInput>(),
            ),
        ]
    }

    pub async fn execute(
        &self,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<Option<CallToolResult>> {
        let result = match name {
            Self::LIST_OWNERS => self.list_owners(deserialize(args)?).await?,
            Self::ADD_OWNER => self.add_owner(deserialize(args)?).await?,
            Self::REMOVE_OWNER => self.remove_owner(deserialize(args)?).await?,
            Self::YANK => self.yank(deserialize(args)?).await?,
            Self::UNYANK => self.unyank(deserialize(args)?).await?,
            Self::LIST_TRUSTPUB => self.list_trustpub(deserialize(args)?).await?,
            Self::CREATE_TRUSTPUB => self.create_trustpub(deserialize(args)?).await?,
            Self::DELETE_TRUSTPUB => self.delete_trustpub(deserialize(args)?).await?,
            Self::SET_TRUSTPUB_ONLY => self.set_trustpub_only(deserialize(args)?).await?,
            _ => return Ok(None),
        };

        Ok(Some(result))
    }

    async fn list_owners(&self, input: CrateNameInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }

        match self.client.list_owners(&input.name).await {
            Ok(owners) => Ok(success_result(format_owners(&owners))),
            Err(error) => Ok(error_result(format!("Failed to list owners: {error}"))),
        }
    }

    async fn add_owner(&self, input: OwnerInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }

        match self
            .client
            .add_owners(&input.name, std::slice::from_ref(&input.owner))
            .await
        {
            Ok(message) => Ok(success_result(message)),
            Err(error) => Ok(error_result(format!("Failed to add owner: {error}"))),
        }
    }

    async fn remove_owner(&self, input: OwnerInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }

        match self
            .client
            .remove_owners(&input.name, std::slice::from_ref(&input.owner))
            .await
        {
            Ok(message) => Ok(success_result(message)),
            Err(error) => Ok(error_result(format!("Failed to remove owner: {error}"))),
        }
    }

    async fn yank(&self, input: YankInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }
        if let Err(error) = validate_version(&input.version) {
            return Ok(error_result(format!("Invalid version: {error}")));
        }

        match self
            .client
            .set_yanked(&input.name, &input.version, true, input.message.as_deref())
            .await
        {
            Ok(()) => Ok(success_result(format!(
                "Yanked {}@{}.",
                input.name, input.version
            ))),
            Err(error) => Ok(error_result(format!("Failed to yank version: {error}"))),
        }
    }

    async fn unyank(&self, input: UnyankInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }
        if let Err(error) = validate_version(&input.version) {
            return Ok(error_result(format!("Invalid version: {error}")));
        }

        match self
            .client
            .set_yanked(&input.name, &input.version, false, None)
            .await
        {
            Ok(()) => Ok(success_result(format!(
                "Unyanked {}@{}.",
                input.name, input.version
            ))),
            Err(error) => Ok(error_result(format!("Failed to unyank version: {error}"))),
        }
    }

    async fn list_trustpub(&self, input: CrateNameInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }

        match self.client.list_github_configs(&input.name).await {
            Ok(configs) => Ok(success_result(format_github_configs(&input.name, &configs))),
            Err(error) => Ok(error_result(format!(
                "Failed to list trusted publishing configs: {error}"
            ))),
        }
    }

    async fn create_trustpub(&self, input: CreateTrustpubInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }
        if let Err(error) = validate_github_name("repository_owner", &input.repository_owner) {
            return Ok(error_result(error.to_string()));
        }
        if let Err(error) = validate_github_name("repository_name", &input.repository_name) {
            return Ok(error_result(error.to_string()));
        }
        if let Err(error) = validate_workflow_filename(&input.workflow_filename) {
            return Ok(error_result(error.to_string()));
        }
        if let Some(environment) = &input.environment
            && let Err(error) = validate_environment(environment)
        {
            return Ok(error_result(error.to_string()));
        }

        let config = NewGithubConfig {
            krate: &input.name,
            repository_owner: &input.repository_owner,
            repository_name: &input.repository_name,
            workflow_filename: &input.workflow_filename,
            environment: input.environment.as_deref(),
        };

        match self.client.create_github_config(&config).await {
            Ok(created) => Ok(success_result(format!(
                "Created trusted publishing config #{} for {}/{} (workflow: {}, environment: {}).",
                created.id,
                created.repository_owner,
                created.repository_name,
                created.workflow_filename,
                created.environment.as_deref().unwrap_or("none")
            ))),
            Err(error) => Ok(error_result(format!(
                "Failed to create trusted publishing config: {error}"
            ))),
        }
    }

    async fn delete_trustpub(&self, input: DeleteTrustpubInput) -> Result<CallToolResult> {
        match self.client.delete_github_config(input.config_id).await {
            Ok(()) => Ok(success_result(format!(
                "Deleted trusted publishing config #{}.",
                input.config_id
            ))),
            Err(error) => Ok(error_result(format!(
                "Failed to delete trusted publishing config: {error}"
            ))),
        }
    }

    async fn set_trustpub_only(&self, input: SetTrustpubOnlyInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }

        match self
            .client
            .set_trustpub_only(&input.name, input.trustpub_only)
            .await
        {
            Ok(()) => Ok(success_result(format!(
                "Set trustpub_only = {} for {}.",
                input.trustpub_only, input.name
            ))),
            Err(error) => Ok(error_result(format!(
                "Failed to update trustpub_only: {error}"
            ))),
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

fn format_owners(owners: &[Owner]) -> String {
    if owners.is_empty() {
        return "This crate has no listed owners.".to_string();
    }

    let mut out = String::from("Owners:\n");
    for owner in owners {
        match &owner.kind {
            Some(kind) if !kind.is_empty() => {
                out.push_str(&format!("- {} ({})\n", owner.login, kind));
            }
            _ => out.push_str(&format!("- {}\n", owner.login)),
        }
    }
    out.trim_end().to_string()
}

fn format_github_configs(crate_name: &str, configs: &[GithubConfig]) -> String {
    if configs.is_empty() {
        return format!("No trusted publishing configs are registered for {crate_name}.");
    }

    let mut out = format!("Trusted publishing configs for {crate_name}:\n");
    for config in configs {
        out.push_str(&format!(
            "- #{} {}/{} (workflow: {}, environment: {})\n",
            config.id,
            config.repository_owner,
            config.repository_name,
            config.workflow_filename,
            config.environment.as_deref().unwrap_or("none")
        ));
    }
    out.trim_end().to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OwnerInput {
    /// Name of the crate
    pub name: String,
    /// Owner login. Use a crates.io username, `crates.io:username`, `github:username`, or `github:org:team`
    pub owner: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct YankInput {
    /// Name of the crate
    pub name: String,
    /// Version to yank, for example `1.2.3`
    pub version: String,
    /// Optional message explaining why the version was yanked
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnyankInput {
    /// Name of the crate
    pub name: String,
    /// Version to unyank, for example `1.2.3`
    pub version: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateTrustpubInput {
    /// Name of the crate
    pub name: String,
    /// GitHub user or organization that owns the repository
    pub repository_owner: String,
    /// GitHub repository name
    pub repository_name: String,
    /// Workflow file name, for example `publish.yml`. Must match the file in .github/workflows
    pub workflow_filename: String,
    /// Optional GitHub Actions environment name configured on the workflow job
    pub environment: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTrustpubInput {
    /// Numeric id of the trusted publishing configuration, as returned by list_trustpub_github_configs
    pub config_id: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetTrustpubOnlyInput {
    /// Name of the crate
    pub name: String,
    /// When true, only trusted publishing tokens may publish new versions
    pub trustpub_only: bool,
}
