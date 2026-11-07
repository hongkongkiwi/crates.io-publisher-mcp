use crate::credentials;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where a crates.io token was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    CratesIoTokenEnv,
    CargoRegistryTokenEnv,
    ConfigFile,
    CredentialsFile,
}

impl TokenSource {
    pub fn description(self) -> &'static str {
        match self {
            Self::CratesIoTokenEnv => "CRATES_IO_TOKEN environment variable",
            Self::CargoRegistryTokenEnv => "CARGO_REGISTRY_TOKEN environment variable",
            Self::ConfigFile => "token file created by `cratesdotio-mcp login`",
            Self::CredentialsFile => "cargo credentials file",
        }
    }
}

/// A crates.io API token.
///
/// The value is never formatted. `Debug` prints a redacted placeholder so that
/// logs and error reports cannot leak the token.
#[derive(Clone)]
pub struct Token {
    value: String,
    source: TokenSource,
    location: Option<PathBuf>,
}

impl Token {
    pub fn new(value: String, source: TokenSource, location: Option<PathBuf>) -> Self {
        Self {
            value,
            source,
            location,
        }
    }

    pub fn expose(&self) -> &str {
        &self.value
    }

    pub fn source(&self) -> TokenSource {
        self.source
    }

    pub fn location(&self) -> Option<&Path> {
        self.location.as_deref()
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token")
            .field("source", &self.source)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Where a crates.io session cookie was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieSource {
    Env,
    ConfigFile,
}

impl CookieSource {
    pub fn description(self) -> &'static str {
        match self {
            Self::Env => "CRATES_IO_COOKIE environment variable",
            Self::ConfigFile => "session file created by `cratesdotio-mcp session`",
        }
    }
}

/// A crates.io `cargo_session` cookie.
///
/// This is full account authority, not a scoped token. It is only loaded in
/// admin mode and never formatted.
#[derive(Clone)]
pub struct Cookie {
    value: String,
    source: CookieSource,
    location: Option<PathBuf>,
}

impl Cookie {
    pub fn new(value: String, source: CookieSource, location: Option<PathBuf>) -> Self {
        Self {
            value,
            source,
            location,
        }
    }

    pub fn expose(&self) -> &str {
        &self.value
    }

    pub fn source(&self) -> CookieSource {
        self.source
    }

    pub fn location(&self) -> Option<&Path> {
        self.location.as_deref()
    }
}

impl std::fmt::Debug for Cookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cookie")
            .field("source", &self.source)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Resolve a token from the process environment, this tool's config file, then
/// the cargo credentials file.
///
/// Precedence is `CRATES_IO_TOKEN`, `CARGO_REGISTRY_TOKEN`,
/// `$CRATES_MCP_CONFIG_DIR/token`, then `$CARGO_HOME/credentials.toml`
/// (`registries.crates-io.token`, then the legacy `registry.token`).
pub fn resolve_token() -> Result<Option<Token>> {
    let env = env_map();
    resolve_token_with(&env, &credentials::cargo_home(), &credentials::config_dir())
}

pub fn resolve_token_with(
    env: &HashMap<String, String>,
    cargo_home: &Path,
    config_dir: &Path,
) -> Result<Option<Token>> {
    const ENV_SOURCES: [(&str, TokenSource); 2] = [
        ("CRATES_IO_TOKEN", TokenSource::CratesIoTokenEnv),
        ("CARGO_REGISTRY_TOKEN", TokenSource::CargoRegistryTokenEnv),
    ];

    for (name, source) in ENV_SOURCES {
        if let Some(value) = env
            .get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return Ok(Some(Token::new(value.to_string(), source, None)));
        }
    }

    if let Some(value) = credentials::stored_token(config_dir)? {
        return Ok(Some(Token::new(
            value,
            TokenSource::ConfigFile,
            Some(config_dir.join("token")),
        )));
    }

    token_from_credentials(cargo_home)
}

/// Resolve a crates.io session cookie from `CRATES_IO_COOKIE`,
/// `CRATES_IO_COOKIE_FILE`, then `$CRATES_MCP_CONFIG_DIR/session`.
pub fn resolve_cookie() -> Result<Option<Cookie>> {
    let env = env_map();
    resolve_cookie_with(&env, &credentials::config_dir())
}

pub fn resolve_cookie_with(
    env: &HashMap<String, String>,
    config_dir: &Path,
) -> Result<Option<Cookie>> {
    if let Some(raw) = env.get("CRATES_IO_COOKIE") {
        let value = normalize_cookie(raw)
            .context("CRATES_IO_COOKIE does not contain a valid cargo_session value")?;
        return Ok(Some(Cookie::new(value, CookieSource::Env, None)));
    }

    if let Some(path) = env.get("CRATES_IO_COOKIE_FILE") {
        let path = PathBuf::from(path);
        let raw = credentials::read_secret(&path)?.ok_or_else(|| {
            anyhow::anyhow!(
                "CRATES_IO_COOKIE_FILE points at {} but the file is missing or empty",
                path.display()
            )
        })?;
        let value = normalize_cookie(&raw).with_context(|| {
            format!(
                "{} does not contain a valid cargo_session value",
                path.display()
            )
        })?;
        return Ok(Some(Cookie::new(
            value,
            CookieSource::ConfigFile,
            Some(path),
        )));
    }

    if let Some(raw) = credentials::stored_cookie(config_dir)? {
        let path = config_dir.join("session");
        let value = normalize_cookie(&raw).with_context(|| {
            format!(
                "{} does not contain a valid cargo_session value",
                path.display()
            )
        })?;
        return Ok(Some(Cookie::new(
            value,
            CookieSource::ConfigFile,
            Some(path),
        )));
    }

    Ok(None)
}

/// Accept either a bare cookie value, `cargo_session=<value>`, or a full
/// `Cookie` header (with or without the `Cookie:` prefix) that contains a
/// `cargo_session` entry.
pub fn normalize_cookie(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let raw = if raw
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
    {
        raw[7..].trim()
    } else {
        raw
    };
    if raw.is_empty() {
        return None;
    }

    let value = if let Some(rest) = raw.strip_prefix("cargo_session=") {
        rest.split(';').next().unwrap_or("").trim()
    } else if raw.contains(';') {
        raw.split(';')
            .find_map(|part| part.trim().strip_prefix("cargo_session=").map(str::trim))?
    } else {
        raw
    };

    valid_cookie_value(value).then(|| value.to_string())
}

fn valid_cookie_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value
            .chars()
            .all(|ch| ch.is_ascii_graphic() && ch != ';' && ch != '"' && ch != '\\')
}

fn env_map() -> HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

fn token_from_credentials(cargo_home: &Path) -> Result<Option<Token>> {
    for file in ["credentials.toml", "credentials"] {
        let path = cargo_home.join(file);
        if !path.is_file() {
            continue;
        }

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading cargo credentials from {}", path.display()))?;
        // Do not include the parser detail: toml errors quote the offending
        // source line, which can contain the token itself.
        let value: toml::Value = toml::from_str(&text).map_err(|_| {
            anyhow::anyhow!(
                "failed to parse cargo credentials file {}; fix its TOML syntax. The parser detail is hidden because it can quote the token.",
                path.display()
            )
        })?;

        let token = value
            .get("registries")
            .and_then(|registries| registries.get("crates-io"))
            .and_then(|registry| registry.get("token"))
            .and_then(|token| token.as_str())
            .or_else(|| {
                value
                    .get("registry")
                    .and_then(|registry| registry.get("token"))
                    .and_then(|token| token.as_str())
            })
            .map(str::trim)
            .filter(|token| !token.is_empty());

        if let Some(token) = token {
            return Ok(Some(Token::new(
                token.to_string(),
                TokenSource::CredentialsFile,
                Some(path),
            )));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn empty_config_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn crates_io_token_wins_over_cargo_token() {
        let config = empty_config_dir();
        let token = resolve_token_with(
            &env(&[
                ("CRATES_IO_TOKEN", "from-crates-io"),
                ("CARGO_REGISTRY_TOKEN", "from-cargo"),
            ]),
            Path::new("/nonexistent"),
            config.path(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(token.expose(), "from-crates-io");
        assert_eq!(token.source(), TokenSource::CratesIoTokenEnv);
    }

    #[test]
    fn cargo_registry_token_is_the_second_choice() {
        let config = empty_config_dir();
        let token = resolve_token_with(
            &env(&[("CARGO_REGISTRY_TOKEN", "from-cargo")]),
            Path::new("/nonexistent"),
            config.path(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(token.expose(), "from-cargo");
        assert_eq!(token.source(), TokenSource::CargoRegistryTokenEnv);
    }

    #[test]
    fn stored_config_token_beats_cargo_credentials() {
        let config = empty_config_dir();
        let cargo_home = tempfile::tempdir().unwrap();
        std::fs::write(
            cargo_home.path().join("credentials.toml"),
            "[registries.crates-io]\ntoken = \"cargo-token\"\n",
        )
        .unwrap();
        credentials::write_secret(&config.path().join("token"), "stored-token").unwrap();

        let token = resolve_token_with(&HashMap::new(), cargo_home.path(), config.path())
            .unwrap()
            .unwrap();

        assert_eq!(token.expose(), "stored-token");
        assert_eq!(token.source(), TokenSource::ConfigFile);
    }

    #[test]
    fn empty_environment_values_are_ignored() {
        let config = empty_config_dir();
        let token = resolve_token_with(
            &env(&[("CRATES_IO_TOKEN", "   "), ("CARGO_REGISTRY_TOKEN", "")]),
            Path::new("/nonexistent"),
            config.path(),
        )
        .unwrap();

        assert!(token.is_none());
    }

    #[test]
    fn reads_registries_crates_io_token_from_credentials_file() {
        let config = empty_config_dir();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials.toml"),
            "[registries.crates-io]\ntoken = \"file-token\"\n",
        )
        .unwrap();

        let token = resolve_token_with(&HashMap::new(), dir.path(), config.path())
            .unwrap()
            .unwrap();

        assert_eq!(token.expose(), "file-token");
        assert_eq!(token.source(), TokenSource::CredentialsFile);
        assert_eq!(
            token.location().unwrap(),
            dir.path().join("credentials.toml")
        );
    }

    #[test]
    fn falls_back_to_legacy_registry_section() {
        let config = empty_config_dir();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials"),
            "[registry]\ntoken = \"legacy-token\"\n",
        )
        .unwrap();

        let token = resolve_token_with(&HashMap::new(), dir.path(), config.path())
            .unwrap()
            .unwrap();

        assert_eq!(token.expose(), "legacy-token");
    }

    #[test]
    fn malformed_credentials_file_is_an_error() {
        let config = empty_config_dir();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("credentials.toml"), "not = [valid").unwrap();

        assert!(resolve_token_with(&HashMap::new(), dir.path(), config.path()).is_err());
    }

    #[test]
    fn malformed_credentials_error_does_not_echo_the_token() {
        let config = empty_config_dir();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials.toml"),
            "token = \"super-secret\" trailing",
        )
        .unwrap();

        let error = resolve_token_with(&HashMap::new(), dir.path(), config.path())
            .unwrap_err()
            .to_string();

        assert!(!error.contains("super-secret"), "{error}");
    }

    #[test]
    fn debug_does_not_leak_the_token() {
        let token = Token::new(
            "super-secret".to_string(),
            TokenSource::CratesIoTokenEnv,
            None,
        );
        let rendered = format!("{token:?}");

        assert!(!rendered.contains("super-secret"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn normalizes_bare_and_header_cookie_forms() {
        assert_eq!(normalize_cookie("abc123").unwrap(), "abc123");
        assert_eq!(normalize_cookie("cargo_session=abc123").unwrap(), "abc123");
        assert_eq!(
            normalize_cookie("foo=bar; cargo_session=abc123; baz=qux").unwrap(),
            "abc123"
        );
        assert_eq!(
            normalize_cookie("cargo_session=abc123; foo=bar").unwrap(),
            "abc123"
        );
        assert_eq!(
            normalize_cookie("Cookie: cargo_session=abc123").unwrap(),
            "abc123"
        );
        assert_eq!(
            normalize_cookie("cookie: cargo_session=abc123; other=x").unwrap(),
            "abc123"
        );
    }

    #[test]
    fn rejects_invalid_cookie_values() {
        assert!(normalize_cookie("").is_none());
        assert!(normalize_cookie("   ").is_none());
        assert!(normalize_cookie("bad\nvalue").is_none());
        assert!(normalize_cookie("bad;value").is_none());
        assert!(normalize_cookie("bad\"value").is_none());
        assert!(normalize_cookie("bad\\value").is_none());
        assert!(normalize_cookie("other=1; session=2").is_none());
    }

    #[test]
    fn a_missing_cookie_file_is_an_error() {
        let config = empty_config_dir();
        let env = env(&[("CRATES_IO_COOKIE_FILE", "/nonexistent/crates-mcp-cookie")]);

        assert!(resolve_cookie_with(&env, config.path()).is_err());
    }

    #[test]
    fn cookie_env_wins_over_the_stored_session() {
        let config = empty_config_dir();
        credentials::write_secret(&config.path().join("session"), "stored-cookie").unwrap();

        let cookie = resolve_cookie_with(
            &env(&[("CRATES_IO_COOKIE", "cargo_session=env-cookie")]),
            config.path(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(cookie.expose(), "env-cookie");
        assert_eq!(cookie.source(), CookieSource::Env);
    }

    #[test]
    fn reads_the_stored_session_file() {
        let config = empty_config_dir();
        credentials::write_secret(&config.path().join("session"), "stored-cookie").unwrap();

        let cookie = resolve_cookie_with(&HashMap::new(), config.path())
            .unwrap()
            .unwrap();

        assert_eq!(cookie.expose(), "stored-cookie");
        assert_eq!(cookie.source(), CookieSource::ConfigFile);
        assert_eq!(cookie.location().unwrap(), config.path().join("session"));
    }

    #[test]
    fn cookie_debug_does_not_leak_the_value() {
        let cookie = Cookie::new("super-secret".to_string(), CookieSource::ConfigFile, None);
        let rendered = format!("{cookie:?}");

        assert!(!rendered.contains("super-secret"));
        assert!(rendered.contains("redacted"));
    }
}
