use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_cratesdotio-mcp");

struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Session {
    fn start(mode: &str, token: Option<&str>) -> Self {
        let mut command = Command::new(BIN);
        command
            .arg("--mode")
            .arg(mode)
            .env_remove("CRATES_MCP_MODE")
            .env_remove("CRATES_IO_TOKEN")
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env(
                "CARGO_HOME",
                std::env::temp_dir().join("crates-mcp-test-home"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(token) = token {
            command.env("CRATES_IO_TOKEN", token);
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

fn tool_names(mode: &str, token: Option<&str>) -> Vec<String> {
    let mut session = Session::start(mode, token);

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
    let tools = tool_names("read-only", None);
    assert_eq!(tools.len(), 7, "{tools:?}");
    assert!(tools.contains(&"search_crates".to_string()));
    assert!(tools.contains(&"crates_auth_status".to_string()));
    assert!(!tools.contains(&"yank_crate_version".to_string()));
    assert!(!tools.contains(&"publish_crate".to_string()));
}

#[test]
fn write_mode_adds_write_tools_but_not_publish() {
    let tools = tool_names("write", Some("fake-token"));
    assert_eq!(tools.len(), 16, "{tools:?}");
    assert!(tools.contains(&"yank_crate_version".to_string()));
    assert!(tools.contains(&"set_trustpub_only".to_string()));
    assert!(!tools.contains(&"publish_crate".to_string()));
}

#[test]
fn publish_mode_exposes_every_tool() {
    let tools = tool_names("publish", Some("fake-token"));
    assert_eq!(tools.len(), 19, "{tools:?}");
    assert!(tools.contains(&"scaffold_crate".to_string()));
    assert!(tools.contains(&"publish_crate".to_string()));
    assert!(tools.contains(&"generate_publish_workflow".to_string()));
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
        .env(
            "CARGO_HOME",
            std::env::temp_dir().join("crates-mcp-test-home"),
        )
        .output()
        .expect("binary runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("needs a crates.io API token"), "{stderr}");
}

#[test]
fn write_mode_refuses_a_non_loopback_http_bind() {
    let output = Command::new(BIN)
        .args(["--mode", "write", "--http", "0", "--bind", "0.0.0.0"])
        .env("CRATES_IO_TOKEN", "fake-token")
        .output()
        .expect("binary runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must bind to loopback"), "{stderr}");
}
