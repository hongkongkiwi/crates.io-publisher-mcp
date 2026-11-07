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

fn build_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        // A redirect could downgrade https to http on the same host, and
        // reqwest keeps the Authorization header across that redirect.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("building crates.io HTTP client")
}

impl WriteClient {
    pub fn new(token: &str, base_url: &str) -> Result<Self> {
        if token.trim().is_empty() {
            anyhow::bail!("crates.io token is empty");
        }
        validate_base_url(base_url)?;

        Ok(Self {
            http: build_http_client()?,
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

/// Client for cookie-only crates.io endpoints.
///
/// The cookie is full account authority. It is only constructed in admin mode.
pub struct SessionClient {
    http: Client,
    cookie: String,
    base_url: String,
}

impl SessionClient {
    pub fn new(cookie: &str, base_url: &str) -> Result<Self> {
        Self::build(cookie, base_url, true)
    }

    /// Loopback http is allowed here only so contract tests can use a mock
    /// server. Production paths always use [`SessionClient::new`].
    #[doc(hidden)]
    pub fn new_for_tests(cookie: &str, base_url: &str) -> Result<Self> {
        Self::build(cookie, base_url, false)
    }

    fn build(cookie: &str, base_url: &str, require_https: bool) -> Result<Self> {
        if cookie.trim().is_empty() {
            anyhow::bail!("crates.io session cookie is empty");
        }
        // The cookie is full account authority. Do not allow the loopback
        // plaintext exception that the scoped-token client uses for tests.
        if require_https && !base_url.starts_with("https://") {
            anyhow::bail!("the crates.io session cookie is only sent over https; got `{base_url}`");
        }
        validate_base_url(base_url)?;

        Ok(Self {
            http: build_http_client()?,
            cookie: cookie.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn with_cookie(&self, request: RequestBuilder) -> RequestBuilder {
        request.header(
            reqwest::header::COOKIE,
            format!("cargo_session={}", self.cookie),
        )
    }

    async fn send<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        let body = self.send_raw(request).await?;
        serde_json::from_str(&body)
            .with_context(|| format!("decoding crates.io response: {}", truncate(&body, 300)))
    }

    async fn send_raw(&self, request: RequestBuilder) -> Result<String> {
        let response = self
            .with_cookie(request)
            .send()
            .await
            .context("sending request to crates.io")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("reading crates.io response body")?;
        let body = redact_response(&body, &self.cookie);

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

    pub async fn whoami(&self) -> Result<MeResponse> {
        self.send(self.http.get(format!("{}/api/v1/me", self.base_url)))
            .await
    }

    pub async fn list_tokens(&self) -> Result<Vec<ApiToken>> {
        let response: TokenList = self
            .send(self.http.get(format!("{}/api/v1/me/tokens", self.base_url)))
            .await?;
        Ok(response.api_tokens)
    }

    pub async fn create_token(
        &self,
        name: &str,
        endpoint_scopes: &[String],
        crate_scopes: &[String],
    ) -> Result<CreatedToken> {
        let request = NewTokenRequest {
            api_token: NewToken {
                name,
                endpoint_scopes: (!endpoint_scopes.is_empty()).then_some(endpoint_scopes),
                crate_scopes: (!crate_scopes.is_empty()).then_some(crate_scopes),
            },
        };

        let response: CreatedTokenResponse = self
            .send(
                self.http
                    .put(format!("{}/api/v1/me/tokens", self.base_url))
                    .json(&request),
            )
            .await?;

        Ok(response.api_token)
    }

    pub async fn revoke_token(&self, id: i32) -> Result<()> {
        self.send_empty(
            self.http
                .delete(format!("{}/api/v1/me/tokens/{id}", self.base_url)),
        )
        .await
    }

    pub async fn delete_crate(&self, name: &str, message: Option<&str>) -> Result<()> {
        let mut request = self
            .http
            .delete(format!("{}/api/v1/crates/{name}", self.base_url));
        if let Some(message) = message {
            request = request.query(&[("message", message)]);
        }
        self.send_empty(request).await
    }
}

#[derive(Debug, Deserialize)]
pub struct MeResponse {
    pub user: PrivateUser,
    #[serde(default)]
    pub owned_crates: Vec<OwnedCrate>,
}

#[derive(Debug, Deserialize)]
pub struct PrivateUser {
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct OwnedCrate {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiToken {
    pub id: i32,
    pub name: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub crate_scopes: Option<Vec<String>>,
    #[serde(default)]
    pub endpoint_scopes: Option<Vec<String>>,
    #[serde(default)]
    pub expired_at: Option<String>,
}

#[derive(Deserialize)]
struct TokenList {
    api_tokens: Vec<ApiToken>,
}

#[derive(Serialize)]
struct NewTokenRequest<'a> {
    api_token: NewToken<'a>,
}

#[derive(Serialize)]
struct NewToken<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    crate_scopes: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    endpoint_scopes: Option<&'a [String]>,
}

#[derive(Debug, Deserialize)]
pub struct CreatedToken {
    pub id: i32,
    pub name: String,
    /// The plaintext token. crates.io only returns it once, at creation.
    pub token: String,
}

#[derive(Deserialize)]
struct CreatedTokenResponse {
    api_token: CreatedToken,
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

/// Turn a write-API error into a message that names the scope the operation
/// needs, so an agent can repair the credential instead of retrying blindly.
pub fn describe_api_error(error: &anyhow::Error, scope: &str) -> String {
    if let Some(api) = error.downcast_ref::<ApiError>() {
        if api.status == StatusCode::FORBIDDEN {
            return format!(
                "{api}. This usually means the token is missing the `{scope}` scope, or it was revoked or expired. Run `crates_auth_status` for the token source, or `list_api_tokens` in admin mode to see recorded scopes."
            );
        }
        if api.status == StatusCode::UNAUTHORIZED {
            return format!(
                "{api}. The token was rejected; it may be revoked, expired, or mistyped."
            );
        }
    }
    format!("{error:#}")
}

/// Session-cookie errors point at the cookie rather than a token scope.
pub fn describe_session_error(error: &anyhow::Error) -> String {
    if let Some(api) = error.downcast_ref::<ApiError>()
        && matches!(api.status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED)
    {
        return format!(
            "{api}. The session cookie was rejected or has expired. Run `cratesdotio-mcp session` again."
        );
    }
    format!("{error:#}")
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

    #[test]
    fn session_client_requires_https() {
        assert!(SessionClient::new("cookie", "http://127.0.0.1:8080").is_err());
        assert!(SessionClient::new("cookie", "https://crates.io").is_ok());
        assert!(SessionClient::new("", "https://crates.io").is_err());
    }

    #[test]
    fn token_creation_body_uses_crates_io_field_names() {
        let scopes = vec!["publish-update".to_string(), "yank".to_string()];
        let request = NewTokenRequest {
            api_token: NewToken {
                name: "ci",
                endpoint_scopes: Some(&scopes),
                crate_scopes: None,
            },
        };
        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(value["api_token"]["name"], "ci");
        assert_eq!(value["api_token"]["endpoint_scopes"][0], "publish-update");
        assert!(value["api_token"].get("crate_scopes").is_none());
    }
}
