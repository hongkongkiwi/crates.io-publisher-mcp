use clap::ValueEnum;

/// How much the server is allowed to change.
///
/// There is deliberately no default. A server that can publish must be started
/// with an explicit choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Mode {
    /// Read-only tools. No token is read and no write tool is exposed.
    ReadOnly,
    /// Read tools plus yank, owner management and trusted publishing configuration.
    Write,
    /// Everything in `write`, plus local scaffolding, workflow generation and publishing.
    Publish,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Write => "write",
            Self::Publish => "publish",
        }
    }

    pub fn allows_write(self) -> bool {
        self >= Self::Write
    }

    pub fn allows_publish(self) -> bool {
        self >= Self::Publish
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn parses_all_mode_names() {
        assert_eq!(Mode::from_str("read-only", true).unwrap(), Mode::ReadOnly);
        assert_eq!(Mode::from_str("write", true).unwrap(), Mode::Write);
        assert_eq!(Mode::from_str("publish", true).unwrap(), Mode::Publish);
    }

    #[test]
    fn rejects_unknown_mode_names() {
        assert!(Mode::from_str("admin", true).is_err());
        assert!(Mode::from_str("", true).is_err());
    }

    #[test]
    fn capability_ladder_is_ordered() {
        assert!(!Mode::ReadOnly.allows_write());
        assert!(Mode::Write.allows_write());
        assert!(!Mode::Write.allows_publish());
        assert!(Mode::Publish.allows_write());
        assert!(Mode::Publish.allows_publish());
    }
}
