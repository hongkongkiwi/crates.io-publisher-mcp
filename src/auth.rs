use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where a crates.io token was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    CratesIoTokenEnv,
    CargoRegistryTokenEnv,
    CredentialsFile,
}

impl TokenSource {
    pub fn description(self) -> &'static str {
        match self {
            Self::CratesIoTokenEnv => "CRATES_IO_TOKEN environment variable",
            Self::CargoRegistryTokenEnv => "CARGO_REGISTRY_TOKEN environment variable",
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

/// Resolve a token from the process environment, then from the cargo
/// credentials file.
///
/// Precedence is `CRATES_IO_TOKEN`, `CARGO_REGISTRY_TOKEN`, then
/// `$CARGO_HOME/credentials.toml` (`registries.crates-io.token`, then the
/// legacy `registry.token`).
pub fn resolve_token() -> Result<Option<Token>> {
    let env: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    resolve_token_with(&env, &cargo_home())
}

pub fn cargo_home() -> PathBuf {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".cargo"))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn resolve_token_with(
    env: &HashMap<String, String>,
    cargo_home: &Path,
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

    token_from_credentials(cargo_home)
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

    #[test]
    fn crates_io_token_wins_over_cargo_token() {
        let token = resolve_token_with(
            &env(&[
                ("CRATES_IO_TOKEN", "from-crates-io"),
                ("CARGO_REGISTRY_TOKEN", "from-cargo"),
            ]),
            Path::new("/nonexistent"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(token.expose(), "from-crates-io");
        assert_eq!(token.source(), TokenSource::CratesIoTokenEnv);
    }

    #[test]
    fn cargo_registry_token_is_the_second_choice() {
        let token = resolve_token_with(
            &env(&[("CARGO_REGISTRY_TOKEN", "from-cargo")]),
            Path::new("/nonexistent"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(token.expose(), "from-cargo");
        assert_eq!(token.source(), TokenSource::CargoRegistryTokenEnv);
    }

    #[test]
    fn empty_environment_values_are_ignored() {
        let token = resolve_token_with(
            &env(&[("CRATES_IO_TOKEN", "   "), ("CARGO_REGISTRY_TOKEN", "")]),
            Path::new("/nonexistent"),
        )
        .unwrap();

        assert!(token.is_none());
    }

    #[test]
    fn reads_registries_crates_io_token_from_credentials_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials.toml"),
            "[registries.crates-io]\ntoken = \"file-token\"\n",
        )
        .unwrap();

        let token = resolve_token_with(&HashMap::new(), dir.path())
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
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials"),
            "[registry]\ntoken = \"legacy-token\"\n",
        )
        .unwrap();

        let token = resolve_token_with(&HashMap::new(), dir.path())
            .unwrap()
            .unwrap();

        assert_eq!(token.expose(), "legacy-token");
    }

    #[test]
    fn malformed_credentials_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("credentials.toml"), "not = [valid").unwrap();

        assert!(resolve_token_with(&HashMap::new(), dir.path()).is_err());
    }

    #[test]
    fn malformed_credentials_error_does_not_echo_the_token() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials.toml"),
            "token = \"super-secret\" trailing",
        )
        .unwrap();

        let error = resolve_token_with(&HashMap::new(), dir.path())
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
}
