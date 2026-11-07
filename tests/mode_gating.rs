use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_cratesdotio-mcp");

fn test_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(name)
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Session {
    fn start_with_cargo_home(
        mode: &str,
        token: Option<&str>,
        cookie: Option<&str>,
        cargo_home: std::path::PathBuf,
    ) -> Self {
        let mut command = Command::new(BIN);
        command
            .arg("--mode")
            .arg(mode)
            .env_remove("CRATES_MCP_MODE")
            .env_remove("CRATES_IO_TOKEN")
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CRATES_IO_COOKIE")
            .env_remove("CRATES_IO_COOKIE_FILE")
            .env("CARGO_HOME", cargo_home)
            .env("CRATES_MCP_CONFIG_DIR", test_dir("crates-mcp-test-config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(token) = token {
            command.env("CRATES_IO_TOKEN", token);
        }
        if let Some(cookie) = cookie {
            command.env("CRATES_IO_COOKIE", cookie);
        }

        let mut child = command.spawn().expect("server binary starts");
        let stdin = child.stdin.take().expect("server stdin");
        let stdout = child.stdout.take().expect("server stdout");

        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });

        Self {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, value: Value) {
        self.stdin
            .write_all(format!("{value}\n").as_bytes())
            .expect("write to server");
        self.stdin.flush().expect("flush to server");
    }

    fn receive(&mut self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(15))
            .expect("server responds before the timeout");
        serde_json::from_str(&line).expect("server sends JSON")
    }

    fn receive_id(&mut self, id: i64) -> Value {
        for _ in 0..10 {
            let value = self.receive();
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                return value;
            }
        }
        panic!("no response with id {id}");
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tool_names(mode: &str, token: Option<&str>, cookie: Option<&str>) -> Vec<String> {
    tool_names_with_cargo_home(mode, token, cookie, test_dir("crates-mcp-test-home"))
}

fn tool_names_with_cargo_home(
    mode: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    cargo_home: std::path::PathBuf,
) -> Vec<String> {
    let mut session = Session::start_with_cargo_home(mode, token, cookie, cargo_home);

    session.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "mode-gating-test", "version": "0" }
        }
    }));
    let init = session.receive_id(1);
    assert!(init.get("result").is_some(), "initialize failed: {init}");

    session.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    session.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }));

    let listing = session.receive_id(2);
    listing["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_string())
        .collect()
}

#[test]
fn read_only_mode_exposes_only_read_tools() {
    let tools = tool_names("read-only", None, None);
    assert_eq!(tools.len(), 7, "{tools:?}");
    assert!(tools.contains(&"search_crates".to_string()));
    assert!(tools.contains(&"crates_auth_status".to_string()));
    assert!(!tools.contains(&"yank_crate_version".to_string()));
    assert!(!tools.contains(&"publish_crate".to_string()));
    assert!(!tools.contains(&"delete_crate".to_string()));
}

#[test]
fn write_mode_adds_write_tools_but_not_publish() {
    let tools = tool_names("write", Some("fake-token"), None);
    assert_eq!(tools.len(), 16, "{tools:?}");
    assert!(tools.contains(&"yank_crate_version".to_string()));
    assert!(tools.contains(&"set_trustpub_only".to_string()));
    assert!(!tools.contains(&"publish_crate".to_string()));
    assert!(!tools.contains(&"delete_crate".to_string()));
}

#[test]
fn publish_mode_exposes_every_tool() {
    let tools = tool_names("publish", Some("fake-token"), None);
    assert_eq!(tools.len(), 19, "{tools:?}");
    assert!(tools.contains(&"scaffold_crate".to_string()));
    assert!(tools.contains(&"publish_crate".to_string()));
    assert!(tools.contains(&"generate_publish_workflow".to_string()));
    assert!(!tools.contains(&"delete_crate".to_string()));
}

#[test]
fn admin_mode_adds_account_tools() {
    let with_token = tool_names("admin", Some("fake-token"), Some("fake-cookie"));
    assert_eq!(with_token.len(), 24, "{with_token:?}");
    assert!(with_token.contains(&"crates_whoami".to_string()));
    assert!(with_token.contains(&"create_api_token".to_string()));
    assert!(with_token.contains(&"delete_crate".to_string()));
    assert!(with_token.contains(&"publish_crate".to_string()));

    let cookie_only = tool_names("admin", None, Some("fake-cookie"));
    assert_eq!(cookie_only.len(), 12, "{cookie_only:?}");
    assert!(cookie_only.contains(&"delete_crate".to_string()));
    assert!(!cookie_only.contains(&"publish_crate".to_string()));
}

#[test]
fn mode_flag_is_required() {
    let output = Command::new(BIN)
        .env_remove("CRATES_MCP_MODE")
        .output()
        .expect("binary runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--mode"), "{stderr}");
}

#[test]
fn write_mode_refuses_to_start_without_a_token() {
    let output = Command::new(BIN)
        .args(["--mode", "write"])
        .env_remove("CRATES_MCP_MODE")
        .env_remove("CRATES_IO_TOKEN")
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env("CARGO_HOME", test_dir("crates-mcp-test-home"))
        .env("CRATES_MCP_CONFIG_DIR", test_dir("crates-mcp-test-config"))
        .output()
        .expect("binary runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("needs a crates.io API token"), "{stderr}");
}

#[test]
fn admin_mode_refuses_to_start_without_a_session_cookie() {
    let output = Command::new(BIN)
        .args(["--mode", "admin"])
        .env_remove("CRATES_MCP_MODE")
        .env_remove("CRATES_IO_COOKIE")
        .env_remove("CRATES_IO_COOKIE_FILE")
        .env_remove("CRATES_IO_TOKEN")
        .env("CARGO_HOME", test_dir("crates-mcp-test-home"))
        .env("CRATES_MCP_CONFIG_DIR", test_dir("crates-mcp-test-config"))
        .output()
        .expect("binary runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("session cookie"), "{stderr}");
}

#[test]
fn admin_mode_ignores_a_broken_cargo_credentials_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("credentials.toml"), "not = [valid").unwrap();

    let tools =
        tool_names_with_cargo_home("admin", None, Some("fake-cookie"), dir.path().to_path_buf());

    assert_eq!(tools.len(), 12, "{tools:?}");
    assert!(tools.contains(&"crates_whoami".to_string()));
}

#[test]
fn admin_mode_refuses_http() {
    let output = Command::new(BIN)
        .args(["--mode", "admin", "--http", "0", "--bind", "127.0.0.1"])
        .env("CRATES_IO_COOKIE", "fake-cookie")
        .env("CRATES_MCP_CONFIG_DIR", test_dir("crates-mcp-test-config"))
        .output()
        .expect("binary runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("stdio-only"), "{stderr}");
}

#[test]
fn write_mode_refuses_a_non_loopback_http_bind() {
    let output = Command::new(BIN)
        .args(["--mode", "write", "--http", "0", "--bind", "0.0.0.0"])
        .env("CRATES_IO_TOKEN", "fake-token")
        .env("CRATES_MCP_CONFIG_DIR", test_dir("crates-mcp-test-config"))
        .output()
        .expect("binary runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must bind to loopback"), "{stderr}");
}
