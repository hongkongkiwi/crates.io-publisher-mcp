use crates_io_publisher_mcp::api::{
    NewGithubConfig, SessionClient, WriteClient, describe_api_error,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn owners(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[tokio::test]
async fn yank_uses_patch_with_the_expected_body() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/crates/demo/1.2.3"))
        .and(header("authorization", "Bearer token"))
        .and(body_json(
            json!({"version": {"yanked": true, "yank_message": "bad"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let client = WriteClient::new("token", &server.uri()).unwrap();
    client
        .set_yanked("demo", "1.2.3", true, Some("bad"))
        .await
        .unwrap();
}

#[tokio::test]
async fn owners_use_the_owners_field_and_parse_logins() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/crates/demo/owners"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"users": [{"login": "alice", "kind": "user"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/crates/demo/owners"))
        .and(body_json(json!({"owners": ["alice"]})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"msg": "invited", "ok": true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/crates/demo/owners"))
        .and(body_json(json!({"owners": ["alice"]})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"msg": "removed", "ok": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = WriteClient::new("token", &server.uri()).unwrap();
    let listed = client.list_owners("demo").await.unwrap();
    assert_eq!(listed[0].login, "alice");
    assert_eq!(listed[0].kind.as_deref(), Some("user"));

    let added = client
        .add_owners("demo", &owners(&["alice"]))
        .await
        .unwrap();
    assert!(added.contains("invited"));
    let removed = client
        .remove_owners("demo", &owners(&["alice"]))
        .await
        .unwrap();
    assert!(removed.contains("removed"));
}

#[tokio::test]
async fn trustpub_contract_matches_crates_io() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/trusted_publishing/github_configs"))
        .and(query_param("crate", "demo"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"github_configs": [], "meta": {"total": 0, "next_page": null}}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/trusted_publishing/github_configs"))
        .and(body_json(json!({
            "github_config": {
                "krate": "demo",
                "repository_owner": "andy",
                "repository_name": "demo",
                "workflow_filename": "publish.yml"
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "github_config": {
                "id": 5,
                "repository_owner": "andy",
                "repository_name": "demo",
                "workflow_filename": "publish.yml",
                "environment": null
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/trusted_publishing/github_configs/5"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/crates/demo"))
        .and(body_json(json!({"krate": {"trustpub_only": true}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let client = WriteClient::new("token", &server.uri()).unwrap();
    assert!(client.list_github_configs("demo").await.unwrap().is_empty());

    let config = NewGithubConfig {
        krate: "demo",
        repository_owner: "andy",
        repository_name: "demo",
        workflow_filename: "publish.yml",
        environment: None,
    };
    let created = client.create_github_config(&config).await.unwrap();
    assert_eq!(created.id, 5);

    client.delete_github_config(5).await.unwrap();
    client.set_trustpub_only("demo", true).await.unwrap();
}

#[tokio::test]
async fn api_errors_keep_the_detail_and_add_a_scope_hint() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/crates/demo/1.2.3"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"errors": [{"detail": "missing or invalid permission"}]})),
        )
        .mount(&server)
        .await;

    let client = WriteClient::new("token", &server.uri()).unwrap();
    let error = client
        .set_yanked("demo", "1.2.3", true, None)
        .await
        .unwrap_err();
    let message = describe_api_error(&error, "yank");

    assert!(
        message.contains("missing or invalid permission"),
        "{message}"
    );
    assert!(message.contains("`yank` scope"), "{message}");
}

#[tokio::test]
async fn session_cookie_is_sent_and_account_calls_parse() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("cookie", "cargo_session=cookie-value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user": {"login": "andy", "email_verified": true},
            "owned_crates": [{"name": "demo"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me/tokens"))
        .and(header("cookie", "cargo_session=cookie-value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "api_tokens": [{
                "id": 1,
                "name": "laptop",
                "endpoint_scopes": ["publish-update"],
                "crate_scopes": null,
                "created_at": "2026-11-01T00:00:00Z",
                "last_used_at": null,
                "expired_at": null
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/me/tokens"))
        .and(body_json(
            json!({"api_token": {"name": "ci", "endpoint_scopes": ["yank"]}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "api_token": {"id": 9, "name": "ci", "token": "plain-text"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/me/tokens/9"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/crates/old-crate"))
        .and(query_param("message", "cleanup"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = SessionClient::new_for_tests("cookie-value", &server.uri()).unwrap();

    let me = client.whoami().await.unwrap();
    assert_eq!(me.user.login, "andy");
    assert_eq!(me.owned_crates[0].name, "demo");

    let tokens = client.list_tokens().await.unwrap();
    assert_eq!(
        tokens[0].endpoint_scopes,
        Some(vec!["publish-update".to_string()])
    );

    let created = client
        .create_token("ci", &owners(&["yank"]), &[])
        .await
        .unwrap();
    assert_eq!(created.id, 9);
    assert_eq!(created.token, "plain-text");

    client.revoke_token(9).await.unwrap();
    client
        .delete_crate("old-crate", Some("cleanup"))
        .await
        .unwrap();
}
