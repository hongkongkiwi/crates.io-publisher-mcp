use anyhow::{Result, bail};

/// Endpoint scopes accepted by crates.io when creating an API token.
pub const ENDPOINT_SCOPES: [&str; 5] = [
    "publish-new",
    "publish-update",
    "trusted-publishing",
    "yank",
    "change-owners",
];

pub fn validate_endpoint_scopes(scopes: &[String]) -> Result<()> {
    if scopes.is_empty() {
        bail!(
            "provide at least one endpoint scope; omitting scopes creates a legacy token with every permission"
        );
    }
    if let Some(invalid) = scopes
        .iter()
        .find(|scope| !ENDPOINT_SCOPES.contains(&scope.as_str()))
    {
        bail!(
            "unknown endpoint scope `{invalid}`. Valid scopes: {}",
            ENDPOINT_SCOPES.join(", ")
        );
    }
    Ok(())
}

pub fn validate_crate_scopes(scopes: &[String]) -> Result<()> {
    if let Some(invalid) = scopes.iter().find(|scope| !valid_crate_scope(scope)) {
        bail!(
            "invalid crate scope `{invalid}`; use a crate name or a prefix pattern such as `serde-*`"
        );
    }
    Ok(())
}

fn valid_crate_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '*'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn accepts_known_endpoint_scopes() {
        validate_endpoint_scopes(&scopes(&["publish-update", "yank"])).unwrap();
    }

    #[test]
    fn rejects_an_empty_endpoint_scope_list() {
        assert!(validate_endpoint_scopes(&[]).is_err());
    }

    #[test]
    fn rejects_unknown_endpoint_scopes() {
        assert!(validate_endpoint_scopes(&scopes(&["publish-updat"])).is_err());
    }

    #[test]
    fn accepts_crate_names_and_prefix_patterns() {
        validate_crate_scopes(&scopes(&["serde", "serde-*", "my_crate"])).unwrap();
    }

    #[test]
    fn rejects_invalid_crate_scopes() {
        assert!(validate_crate_scopes(&scopes(&[""])).is_err());
        assert!(validate_crate_scopes(&scopes(&["serde/*"])).is_err());
        assert!(validate_crate_scopes(&scopes(&["serde crate"])).is_err());
    }
}
