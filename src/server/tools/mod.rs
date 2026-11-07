use crate::mode::Mode;
use schemars::JsonSchema;
use std::sync::Arc;

pub mod admin;
pub mod dependencies;
pub mod documentation;
pub mod info;
pub mod publish;
pub mod search;
pub mod versions;
pub mod write;

pub use admin::AdminTools;
pub use dependencies::GetCrateDependenciesTool;
pub use documentation::GetCrateDocumentationTool;
pub use info::GetCrateInfoTool;
pub use publish::PublishTools;
pub use search::SearchCratesTool;
pub use versions::GetCrateVersionsTool;
pub use write::WriteTools;

/// Read-only tools implemented directly by the server rather than their own struct.
pub const CHECK_CRATE_NAME: &str = "check_crate_name";
pub const AUTH_STATUS: &str = "crates_auth_status";

/// The minimum mode required to call a tool, or `None` if the name is unknown.
pub fn required_mode(name: &str) -> Option<Mode> {
    match name {
        SearchCratesTool::NAME
        | GetCrateInfoTool::NAME
        | GetCrateVersionsTool::NAME
        | GetCrateDependenciesTool::NAME
        | GetCrateDocumentationTool::NAME
        | CHECK_CRATE_NAME
        | AUTH_STATUS => Some(Mode::ReadOnly),
        WriteTools::LIST_OWNERS
        | WriteTools::ADD_OWNER
        | WriteTools::REMOVE_OWNER
        | WriteTools::YANK
        | WriteTools::UNYANK
        | WriteTools::LIST_TRUSTPUB
        | WriteTools::CREATE_TRUSTPUB
        | WriteTools::DELETE_TRUSTPUB
        | WriteTools::SET_TRUSTPUB_ONLY => Some(Mode::Write),
        PublishTools::SCAFFOLD | PublishTools::PUBLISH | PublishTools::WORKFLOW => {
            Some(Mode::Publish)
        }
        AdminTools::WHOAMI
        | AdminTools::LIST_TOKENS
        | AdminTools::CREATE_TOKEN
        | AdminTools::REVOKE_TOKEN
        | AdminTools::DELETE_CRATE => Some(Mode::Admin),
        _ => None,
    }
}

pub(crate) fn schema_for<T: JsonSchema>() -> Arc<serde_json::Map<String, serde_json::Value>> {
    let schema = schemars::schema_for!(T);
    match serde_json::to_value(schema).unwrap() {
        serde_json::Value::Object(m) => Arc::new(m),
        _ => panic!("schema should be an object"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_a_mode_to_every_known_tool() {
        assert_eq!(required_mode(SearchCratesTool::NAME), Some(Mode::ReadOnly));
        assert_eq!(required_mode(CHECK_CRATE_NAME), Some(Mode::ReadOnly));
        assert_eq!(required_mode(AUTH_STATUS), Some(Mode::ReadOnly));
        assert_eq!(required_mode(WriteTools::LIST_OWNERS), Some(Mode::Write));
        assert_eq!(required_mode(WriteTools::YANK), Some(Mode::Write));
        assert_eq!(
            required_mode(WriteTools::SET_TRUSTPUB_ONLY),
            Some(Mode::Write)
        );
        assert_eq!(required_mode(PublishTools::SCAFFOLD), Some(Mode::Publish));
        assert_eq!(required_mode(PublishTools::PUBLISH), Some(Mode::Publish));
        assert_eq!(required_mode(PublishTools::WORKFLOW), Some(Mode::Publish));
        assert_eq!(required_mode(AdminTools::WHOAMI), Some(Mode::Admin));
        assert_eq!(required_mode(AdminTools::CREATE_TOKEN), Some(Mode::Admin));
        assert_eq!(required_mode(AdminTools::DELETE_CRATE), Some(Mode::Admin));
        assert_eq!(required_mode("not_a_tool"), None);
    }
}
