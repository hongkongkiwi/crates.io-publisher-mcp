use super::schema_for;
use crate::api::{ApiToken, SessionClient, describe_session_error};
use crate::audit;
use crate::credentials;
use crate::ops::validate_crate_name;
use crate::scopes::{validate_crate_scopes, validate_endpoint_scopes};
use anyhow::{Result, anyhow};
use rmcp::model::{CallToolResult, Content, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Account-level tools that need a crates.io session cookie.
pub struct AdminTools {
    client: Arc<SessionClient>,
    config_dir: PathBuf,
}

impl AdminTools {
    pub const WHOAMI: &'static str = "crates_whoami";
    pub const LIST_TOKENS: &'static str = "list_api_tokens";
    pub const CREATE_TOKEN: &'static str = "create_api_token";
    pub const REVOKE_TOKEN: &'static str = "revoke_api_token";
    pub const DELETE_CRATE: &'static str = "delete_crate";

    pub fn new(client: Arc<SessionClient>, config_dir: PathBuf) -> Self {
        Self { client, config_dir }
    }

    pub fn tools(&self) -> Vec<Tool> {
        vec![
            Tool::new(
                Self::WHOAMI,
                "Show the crates.io account behind the stored session cookie, including whether the email is verified and which crates the account owns. Requires admin mode.",
                schema_for::<EmptyInput>(),
            ),
            Tool::new(
                Self::LIST_TOKENS,
                "List active crates.io API tokens for the account, with their scopes and last-used time. Token values are never shown. Requires admin mode.",
                schema_for::<EmptyInput>(),
            ),
            Tool::new(
                Self::CREATE_TOKEN,
                "Create a scoped crates.io API token using the stored session cookie. By default the value is written to this tool's token file and is not returned, so it never enters the transcript. Set reveal = true to return the value instead. Requires admin mode.",
                schema_for::<CreateTokenInput>(),
            ),
            Tool::new(
                Self::REVOKE_TOKEN,
                "Revoke a crates.io API token by id. Requires admin mode and confirm set to `revoke <id>`.",
                schema_for::<RevokeTokenInput>(),
            ),
            Tool::new(
                Self::DELETE_CRATE,
                "Permanently delete a crate from crates.io. This is irreversible and only allowed within crates.io's rules: less than 72 hours old, or a single owner with under 1000 downloads per month and no reverse dependencies. Requires admin mode and confirm set to `delete <name>`.",
                schema_for::<DeleteCrateInput>(),
            ),
        ]
    }

    pub async fn execute(
        &self,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<Option<CallToolResult>> {
        let result = match name {
            Self::WHOAMI => self.whoami().await?,
            Self::LIST_TOKENS => self.list_tokens().await?,
            Self::CREATE_TOKEN => self.create_token(deserialize(args)?).await?,
            Self::REVOKE_TOKEN => self.revoke_token(deserialize(args)?).await?,
            Self::DELETE_CRATE => self.delete_crate(deserialize(args)?).await?,
            _ => return Ok(None),
        };

        Ok(Some(result))
    }

    async fn whoami(&self) -> Result<CallToolResult> {
        match self.client.whoami().await {
            Ok(me) => Ok(success_result(format_whoami(&me))),
            Err(error) => Ok(error_result(format!(
                "Failed to read the account: {}",
                describe_session_error(&error)
            ))),
        }
    }

    async fn list_tokens(&self) -> Result<CallToolResult> {
        match self.client.list_tokens().await {
            Ok(tokens) => Ok(success_result(format_tokens(&tokens))),
            Err(error) => Ok(error_result(format!(
                "Failed to list tokens: {}",
                describe_session_error(&error)
            ))),
        }
    }

    async fn create_token(&self, input: CreateTokenInput) -> Result<CallToolResult> {
        if input.name.trim().is_empty() {
            return Ok(error_result("Token name cannot be empty.".to_string()));
        }
        if let Err(error) = validate_endpoint_scopes(&input.endpoint_scopes) {
            return Ok(error_result(error.to_string()));
        }
        if let Err(error) = validate_crate_scopes(&input.crate_scopes) {
            return Ok(error_result(error.to_string()));
        }
        if !input.store && !input.reveal {
            return Ok(error_result(
                "Refusing to create a token that would be neither stored nor returned.".to_string(),
            ));
        }

        let created = match self
            .client
            .create_token(&input.name, &input.endpoint_scopes, &input.crate_scopes)
            .await
        {
            Ok(created) => created,
            Err(error) => {
                self.audited("create-api-token", &input.name, false);
                return Ok(error_result(format!(
                    "Failed to create token: {}",
                    describe_session_error(&error)
                )));
            }
        };
        self.audited("create-api-token", &input.name, true);

        let mut message = format!(
            "Created API token #{} ({}) with scopes: {}.",
            created.id,
            created.name,
            input.endpoint_scopes.join(", ")
        );

        if input.store {
            let path = self.config_dir.join("token");
            if let Err(error) = credentials::write_secret(&path, &created.token) {
                let mut failure = format!(
                    "Token #{} was created but could not be stored: {error}.",
                    created.id
                );
                if input.reveal {
                    failure.push_str(&format!("\nPlaintext token:\n{}", created.token));
                } else {
                    failure.push_str(
                        " It is now unrecoverable; revoke it and create another, or retry with reveal = true.",
                    );
                }
                return Ok(error_result(failure));
            }
            let meta = credentials::TokenMeta::for_token(
                &created.token,
                created.id,
                created.name.clone(),
                input.endpoint_scopes.clone(),
                input.crate_scopes.clone(),
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(error) = credentials::write_token_meta(&self.config_dir, &meta) {
                message.push_str(&format!(
                    "\nwarning: could not record the token scopes: {error}"
                ));
            }
            message.push_str(&format!(
                "\nStored at {} with owner-only permissions. Any previous stored token was replaced. The running server keeps the token it loaded at startup, so restart it to use the new one.",
                path.display()
            ));
        }

        if input.reveal {
            message.push_str(&format!(
                "\nPlaintext token, shown once and now in the transcript:\n{}",
                created.token
            ));
        } else {
            message.push_str("\nThe token value is not included in this result.");
        }

        Ok(success_result(message))
    }

    async fn revoke_token(&self, input: RevokeTokenInput) -> Result<CallToolResult> {
        let expected = format!("revoke {}", input.config_id);
        if input.confirm != expected {
            return Ok(error_result(format!(
                "confirm must be exactly `{expected}`"
            )));
        }

        let result = self.client.revoke_token(input.config_id).await;
        self.audited(
            "revoke-api-token",
            &input.config_id.to_string(),
            result.is_ok(),
        );

        match result {
            Ok(()) => Ok(success_result(format!(
                "Revoked API token #{}.",
                input.config_id
            ))),
            Err(error) => Ok(error_result(format!(
                "Failed to revoke token: {}",
                describe_session_error(&error)
            ))),
        }
    }

    async fn delete_crate(&self, input: DeleteCrateInput) -> Result<CallToolResult> {
        if let Err(error) = validate_crate_name(&input.name) {
            return Ok(error_result(format!("Invalid crate name: {error}")));
        }

        let expected = format!("delete {}", input.name);
        if input.confirm != expected {
            return Ok(error_result(format!(
                "confirm must be exactly `{expected}`. Deletion is permanent."
            )));
        }

        let result = self
            .client
            .delete_crate(&input.name, input.message.as_deref())
            .await;
        self.audited("delete-crate", &input.name, result.is_ok());

        match result {
            Ok(()) => Ok(success_result(format!(
                "Deleted crate `{}`. crates.io reserves the name for 24 hours, and the crate cannot be restored.",
                input.name
            ))),
            Err(error) => Ok(error_result(format!(
                "Failed to delete crate: {}",
                describe_session_error(&error)
            ))),
        }
    }

    fn audited(&self, action: &str, target: &str, ok: bool) {
        audit::record_or_warn(
            &self.config_dir,
            "admin",
            action,
            target,
            if ok { "ok" } else { "error" },
            None,
        );
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

fn format_whoami(me: &crate::api::MeResponse) -> String {
    let mut out = format!("login: {}", me.user.login);
    if let Some(name) = &me.user.name {
        out.push_str(&format!("\nname: {name}"));
    }
    if let Some(email) = &me.user.email {
        let state = if me.user.email_verified {
            "verified"
        } else {
            "unverified"
        };
        out.push_str(&format!("\nemail: {email} ({state})"));
    }
    out.push_str(&format!("\nemail verified: {}", me.user.email_verified));

    if me.owned_crates.is_empty() {
        out.push_str("\nowned crates: none");
    } else {
        let names: Vec<&str> = me
            .owned_crates
            .iter()
            .map(|krate| krate.name.as_str())
            .collect();
        out.push_str(&format!(
            "\nowned crates ({}): {}",
            names.len(),
            names.join(", ")
        ));
    }

    out
}

fn format_tokens(tokens: &[ApiToken]) -> String {
    if tokens.is_empty() {
        return "No active API tokens.".to_string();
    }

    let mut out = String::from("Active API tokens:\n");
    for token in tokens {
        let scopes = match &token.endpoint_scopes {
            Some(scopes) if !scopes.is_empty() => scopes.join(", "),
            Some(_) => "none".to_string(),
            None => "legacy (all permissions)".to_string(),
        };
        let crates = match &token.crate_scopes {
            Some(scopes) if !scopes.is_empty() => scopes.join(", "),
            Some(_) => "none".to_string(),
            None => "all crates".to_string(),
        };
        let last_used = token.last_used_at.as_deref().unwrap_or("never");
        let expires = token.expired_at.as_deref().unwrap_or("never");

        out.push_str(&format!(
            "- #{} `{}` scopes={} crates={} created={} last_used={} expires={}\n",
            token.id,
            token.name,
            scopes,
            crates,
            token.created_at.as_deref().unwrap_or("unknown"),
            last_used,
            expires
        ));
    }

    out.trim_end().to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EmptyInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateTokenInput {
    /// Human-readable token name, for example `laptop-mcp`
    pub name: String,
    /// Endpoint scopes. At least one is required. Valid values: publish-new, publish-update, trusted-publishing, yank, change-owners
    #[serde(default)]
    pub endpoint_scopes: Vec<String>,
    /// Optional crate name or prefix patterns, for example `serde` or `serde-*`
    #[serde(default)]
    pub crate_scopes: Vec<String>,
    /// Return the plaintext token in the tool result. It will be in the transcript
    #[serde(default)]
    pub reveal: bool,
    /// Store the token in this tool's token file, replacing any previous one
    #[serde(default = "default_true")]
    pub store: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RevokeTokenInput {
    /// Numeric id of the token, as returned by list_api_tokens
    pub config_id: i32,
    /// Type `revoke <id>` exactly
    pub confirm: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteCrateInput {
    /// Name of the crate to delete
    pub name: String,
    /// Type `delete <name>` exactly. Deletion is permanent
    pub confirm: String,
    /// Optional reason stored with the deletion record
    pub message: Option<String>,
}
