use crate::USER_AGENT;
use anyhow::{Context, Result};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::time::Duration;

/// An error response from crates.io.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub detail: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "crates.io returned HTTP {}: {}",
            self.status.as_u16(),
            self.detail
        )
    }
}

impl std::error::Error for ApiError {}

/// Authenticated client for the crates.io write API.
pub struct WriteClient {
    http: Client,
    token: String,
    base_url: String,
}

impl WriteClient {
    pub fn new(token: &str, base_url: &str) -> Result<Self> {
        if token.trim().is_empty() {
            anyhow::bail!("crates.io token is empty");
        }
        validate_base_url(base_url)?;

        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            // A redirect could downgrade https to http on the same host, and
            // reqwest keeps the Authorization header across that redirect.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("building crates.io HTTP client")?;

        Ok(Self {
            http,
            token: token.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn crate_url(&self, name: &str) -> String {
        format!("{}/api/v1/crates/{name}", self.base_url)
    }

    async fn send<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        let body = self.send_raw(request).await?;
        serde_json::from_str(&body)
            .with_context(|| format!("decoding crates.io response: {}", truncate(&body, 300)))
    }

    async fn send_raw(&self, request: RequestBuilder) -> Result<String> {
        let response = request
            .bearer_auth(&self.token)
            .send()
            .await
            .context("sending request to crates.io")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("reading crates.io response body")?;
        // A misconfigured or hostile base URL could echo the Authorization
        // header back in an error detail or success message.
        let body = redact_response(&body, &self.token);

        if !status.is_success() {
            return Err(ApiError {
                status,
                detail: error_detail(&body),
            }
            .into());
        }

        Ok(body)
    }

    async fn send_empty(&self, request: RequestBuilder) -> Result<()> {
        self.send_raw(request).await.map(|_| ())
    }

    pub async fn set_yanked(
        &self,
        name: &str,
        version: &str,
        yanked: bool,
        message: Option<&str>,
    ) -> Result<()> {
        let body = yank_body(yanked, message);
        let url = format!("{}/{version}", self.crate_url(name));
        self.send_empty(self.http.patch(url).json(&body)).await
    }

    pub async fn list_owners(&self, name: &str) -> Result<Vec<Owner>> {
        let response: OwnersResponse = self
            .send(self.http.get(format!("{}/owners", self.crate_url(name))))
            .await?;
        Ok(response.users)
    }

    pub async fn add_owners(&self, name: &str, owners: &[String]) -> Result<String> {
        let body = json!({ "owners": owners });
        let response: ModifyResponse = self
            .send(
                self.http
                    .put(format!("{}/owners", self.crate_url(name)))
                    .json(&body),
            )
            .await?;
        Ok(response.msg)
    }

    pub async fn remove_owners(&self, name: &str, owners: &[String]) -> Result<String> {
        let body = json!({ "owners": owners });
        let response: ModifyResponse = self
            .send(
                self.http
                    .delete(format!("{}/owners", self.crate_url(name)))
                    .json(&body),
            )
            .await?;
        Ok(response.msg)
    }

    pub async fn list_github_configs(&self, name: &str) -> Result<Vec<GithubConfig>> {
        let response: GithubConfigList = self
            .send(
                self.http
                    .get(format!(
                        "{}/api/v1/trusted_publishing/github_configs",
                        self.base_url
                    ))
                    .query(&[("crate", name)]),
            )
            .await?;
        Ok(response.github_configs)
    }

    pub async fn create_github_config(&self, config: &NewGithubConfig<'_>) -> Result<GithubConfig> {
        let body = CreateGithubConfigRequest {
            github_config: config,
        };
        let response: CreateGithubConfigResponse = self
            .send(
                self.http
                    .post(format!(
                        "{}/api/v1/trusted_publishing/github_configs",
                        self.base_url
                    ))
                    .json(&body),
            )
            .await?;
        Ok(response.github_config)
    }

    pub async fn delete_github_config(&self, id: i32) -> Result<()> {
        self.send_empty(self.http.delete(format!(
            "{}/api/v1/trusted_publishing/github_configs/{id}",
            self.base_url
        )))
        .await
    }

    pub async fn set_trustpub_only(&self, name: &str, trustpub_only: bool) -> Result<()> {
        let body = json!({ "krate": { "trustpub_only": trustpub_only } });
        self.send_empty(self.http.patch(self.crate_url(name)).json(&body))
            .await
    }
}

fn yank_body(yanked: bool, message: Option<&str>) -> Value {
    let mut version = json!({ "yanked": yanked });
    if let Some(message) = message {
        version["yank_message"] = json!(message);
    }
    json!({ "version": version })
}

#[derive(Debug, Clone, Deserialize)]
pub struct Owner {
    pub login: String,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Deserialize)]
struct OwnersResponse {
    users: Vec<Owner>,
}

#[derive(Deserialize)]
struct ModifyResponse {
    msg: String,
    #[allow(dead_code)]
    ok: bool,
}

#[derive(Debug, Serialize)]
pub struct NewGithubConfig<'a> {
    pub krate: &'a str,
    pub repository_owner: &'a str,
    pub repository_name: &'a str,
    pub workflow_filename: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<&'a str>,
}

#[derive(Serialize)]
struct CreateGithubConfigRequest<'a> {
    github_config: &'a NewGithubConfig<'a>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubConfig {
    pub id: i32,
    pub repository_owner: String,
    pub repository_name: String,
    pub workflow_filename: String,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Deserialize)]
struct CreateGithubConfigResponse {
    github_config: GithubConfig,
}

#[derive(Deserialize)]
struct GithubConfigList {
    github_configs: Vec<GithubConfig>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    errors: Vec<ErrorItem>,
}

#[derive(Deserialize)]
struct ErrorItem {
    detail: String,
}

fn error_detail(body: &str) -> String {
    if let Ok(envelope) = serde_json::from_str::<ErrorEnvelope>(body)
        && !envelope.errors.is_empty()
    {
        return envelope
            .errors
            .into_iter()
            .map(|error| error.detail)
            .collect::<Vec<_>>()
            .join("; ");
    }

    let body = body.trim();
    if body.is_empty() {
        "empty response body".to_string()
    } else {
        truncate(body, 500)
    }
}

fn redact_response(body: &str, token: &str) -> String {
    if token.is_empty() {
        body.to_string()
    } else {
        body.replace(token, "<redacted-token>")
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let mut truncated: String = value.chars().take(max_chars).collect();
        truncated.push_str("...");
        truncated
    }
}

/// The token is sent as a bearer credential, so refuse to send it in
/// plaintext to a remote host. Loopback HTTP stays available for tests.
pub fn validate_base_url(base_url: &str) -> Result<()> {
    let url = url::Url::parse(base_url)
        .with_context(|| format!("invalid CRATES_MCP_BASE_URL `{base_url}`"))?;

    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = url.host().context("CRATES_MCP_BASE_URL has no host")?;
            let loopback = match host {
                url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
                url::Host::Ipv4(ip) => ip.is_loopback(),
                url::Host::Ipv6(ip) => ip.is_loopback(),
            };
            if loopback {
                Ok(())
            } else {
                anyhow::bail!(
                    "CRATES_MCP_BASE_URL must use https; plaintext http is only allowed for loopback test servers, not `{base_url}`"
                )
            }
        }
        scheme => {
            anyhow::bail!("CRATES_MCP_BASE_URL must use https or loopback http, not `{scheme}`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crates_io_error_envelopes() {
        let body = r#"{"errors":[{"detail":"missing or empty metadata"}]}"#;
        assert_eq!(error_detail(body), "missing or empty metadata");
    }

    #[test]
    fn joins_multiple_error_details() {
        let body = r#"{"errors":[{"detail":"first"},{"detail":"second"}]}"#;
        assert_eq!(error_detail(body), "first; second");
    }

    #[test]
    fn falls_back_to_the_raw_body() {
        assert_eq!(error_detail("  upstream exploded  "), "upstream exploded");
        assert_eq!(error_detail(""), "empty response body");
    }

    #[test]
    fn yank_body_omits_absent_message() {
        assert_eq!(
            yank_body(true, None),
            json!({ "version": { "yanked": true } })
        );
    }

    #[test]
    fn yank_body_includes_message_when_present() {
        assert_eq!(
            yank_body(false, Some("fixed in 1.2.4")),
            json!({ "version": { "yanked": false, "yank_message": "fixed in 1.2.4" } })
        );
    }

    #[test]
    fn truncates_by_characters_not_bytes() {
        assert_eq!(truncate("abcd", 4), "abcd");
        assert_eq!(truncate("abcde", 4), "abcd...");
        assert_eq!(truncate("日本語です", 2), "日本...");
    }

    #[test]
    fn base_url_requires_https_or_loopback() {
        validate_base_url("https://crates.io").unwrap();
        validate_base_url("http://127.0.0.1:8080").unwrap();
        validate_base_url("http://localhost:8080").unwrap();
        validate_base_url("http://[::1]:8080").unwrap();
        assert!(validate_base_url("http://crates.io").is_err());
        assert!(validate_base_url("ftp://crates.io").is_err());
        assert!(validate_base_url("http://localhost.attacker.example").is_err());
        assert!(validate_base_url("http://127.0.0.1.attacker.example").is_err());
    }

    #[test]
    fn redacts_echoed_credentials_without_breaking_json() {
        let body = r#"{"errors":[{"detail":"bad token abc123"}]}"#;
        let redacted = redact_response(body, "abc123");

        assert!(!redacted.contains("abc123"));
        assert!(redacted.contains("<redacted-token>"));
        assert!(serde_json::from_str::<serde_json::Value>(&redacted).is_ok());
    }
}
