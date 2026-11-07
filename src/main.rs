use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use cratesdotio_mcp::api::SessionClient;
use cratesdotio_mcp::auth;
use cratesdotio_mcp::credentials;
use cratesdotio_mcp::scopes::{validate_crate_scopes, validate_endpoint_scopes};
use cratesdotio_mcp::{CratesIoServer, DEFAULT_BASE_URL, Mode, ServerConfig};
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Access mode: read-only, write, publish, or admin. Required when no
    /// subcommand is given.
    #[arg(long, value_enum, env = "CRATES_MCP_MODE")]
    mode: Option<Mode>,

    /// Start the HTTP/SSE server on PORT. If no port is given, it defaults to 3000.
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "3000")]
    http: Option<u16>,

    /// Address to bind the HTTP server to. Defaults to all interfaces in read-only
    /// mode and loopback in every other mode.
    #[arg(long, value_name = "ADDR")]
    bind: Option<IpAddr>,
}

#[derive(Subcommand)]
enum Command {
    /// Store a crates.io API token for this tool
    Login,
    /// Store a crates.io session cookie for admin mode
    Session,
    /// Create a scoped API token with the stored session cookie
    MintToken {
        /// Token name, for example `laptop-mcp`
        #[arg(long)]
        name: String,
        /// Comma-separated endpoint scopes: publish-new, publish-update,
        /// trusted-publishing, yank, change-owners
        #[arg(long, value_delimiter = ',')]
        scopes: Vec<String>,
        /// Comma-separated crate name or prefix patterns, for example `serde,serde-*`
        #[arg(long, value_delimiter = ',')]
        crate_scopes: Vec<String>,
        /// Print the token value to stdout
        #[arg(long)]
        reveal: bool,
        /// Do not store the created token
        #[arg(long)]
        no_store: bool,
    },
    /// Show which credentials are stored, without printing values
    Status,
    /// Remove stored credentials
    Logout {
        /// Remove only the stored API token
        #[arg(long)]
        token: bool,
        /// Remove only the stored session cookie
        #[arg(long)]
        session: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    if let Some(command) = cli.command {
        return run_command(command).await;
    }

    let mode = cli
        .mode
        .ok_or_else(|| anyhow!("--mode is required when no subcommand is given"))?;
    let config = ServerConfig::new(mode)?;

    if let Some(port) = cli.http {
        let bind = cli.bind.unwrap_or_else(|| {
            if mode == Mode::ReadOnly {
                IpAddr::V6(Ipv6Addr::UNSPECIFIED)
            } else {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            }
        });

        // A write-capable server reachable from the network holds an open
        // publishing credential. Keep those modes on the loopback interface.
        if mode != Mode::ReadOnly && !bind.is_loopback() {
            bail!(
                "`--mode {mode}` over HTTP must bind to loopback, not {bind}. Use stdio for \
                 write access, or run the HTTP server in read-only mode."
            );
        }

        let address = std::net::SocketAddr::new(bind, port);
        tracing::info!("Starting crates.io MCP server in {mode} mode on {address}");

        let server_config = config.clone();
        let server_factory =
            move || CratesIoServer::new(server_config.clone()).map_err(std::io::Error::other);
        let session_manager = std::sync::Arc::new(LocalSessionManager::default());
        let http_config = StreamableHttpServerConfig::default();

        let service = StreamableHttpService::new(server_factory, session_manager, http_config);
        let app = axum::Router::new().fallback_service(service);

        let listener = tokio::net::TcpListener::bind(address).await?;
        tracing::info!("Server running on http://{address}");

        axum::serve(listener, app).await?;
    } else {
        tracing::info!("Starting crates.io MCP server in {mode} mode over stdio");

        let server = CratesIoServer::new(config)?;
        let service = server.serve(stdio()).await?;

        tracing::info!("Server running, waiting for requests...");
        service.waiting().await?;
    }

    tracing::info!("Server shutting down");
    Ok(())
}

async fn run_command(command: Command) -> Result<()> {
    match command {
        Command::Login => store_token(),
        Command::Session => store_session().await,
        Command::MintToken {
            name,
            scopes,
            crate_scopes,
            reveal,
            no_store,
        } => mint_token(name, scopes, crate_scopes, reveal, no_store).await,
        Command::Status => print_status(),
        Command::Logout { token, session } => logout(token, session),
    }
}

fn store_token() -> Result<()> {
    println!("Create a token at https://crates.io/settings/tokens.");
    println!("Tick only the scopes you need, then paste the value below. Input is hidden.");
    open_browser("https://crates.io/settings/tokens");

    let token = prompt_secret("API token: ")?;
    let path = credentials::token_path();
    credentials::write_secret(&path, &token)?;
    println!("Stored token at {} (mode 0600).", path.display());
    Ok(())
}

async fn store_session() -> Result<()> {
    println!("The cargo_session cookie grants full account authority.");
    println!("It is stored with mode 0600 and only loaded in admin mode.");
    println!("1. Sign in at https://crates.io/settings/tokens.");
    println!("2. Open browser devtools, then Application, then Cookies, then https://crates.io.");
    println!("3. Copy the `cargo_session` cookie value and paste it below. Input is hidden.");
    open_browser("https://crates.io/settings/tokens");

    let raw = prompt_secret("cargo_session cookie: ")?;
    let value = auth::normalize_cookie(&raw).context("not a valid cargo_session cookie value")?;

    let base_url =
        std::env::var("CRATES_MCP_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let client = SessionClient::new(&value, &base_url)?;
    let me = client
        .whoami()
        .await
        .context("the cookie was not accepted; nothing was stored")?;
    println!("Signed in as {}.", me.user.login);

    let path = credentials::session_path();
    credentials::write_secret(&path, &value)?;
    println!("Stored session at {} (mode 0600).", path.display());
    Ok(())
}

async fn mint_token(
    name: String,
    scopes: Vec<String>,
    crate_scopes: Vec<String>,
    reveal: bool,
    no_store: bool,
) -> Result<()> {
    validate_endpoint_scopes(&scopes)?;
    validate_crate_scopes(&crate_scopes)?;
    if no_store && !reveal {
        bail!("refusing to create a token that would be neither stored nor printed");
    }

    let cookie = auth::resolve_cookie()?
        .context("no session cookie found; run `cratesdotio-mcp session` first")?;
    let base_url =
        std::env::var("CRATES_MCP_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let client = SessionClient::new(cookie.expose(), &base_url)?;

    let created = client.create_token(&name, &scopes, &crate_scopes).await?;
    println!("Created token #{} ({}).", created.id, created.name);

    if !no_store {
        let path = credentials::token_path();
        if let Err(error) = credentials::write_secret(&path, &created.token) {
            if reveal {
                println!("Token: {}", created.token);
            }
            bail!(
                "token #{} was created but could not be stored: {error}. It remains active; revoke it if you cannot recover the value.",
                created.id
            );
        }
        println!(
            "Stored at {} (mode 0600). Any previous stored token was replaced.",
            path.display()
        );
    }

    if reveal {
        println!("Token: {}", created.token);
    } else {
        println!("The token value was not printed.");
    }

    Ok(())
}

fn print_status() -> Result<()> {
    match auth::resolve_token()? {
        Some(token) => {
            println!("token: present ({})", token.source().description());
            if let Some(path) = token.location() {
                println!("  location: {}", path.display());
            }
        }
        None => println!("token: none"),
    }

    match auth::resolve_cookie()? {
        Some(cookie) => {
            println!(
                "session cookie: present ({})",
                cookie.source().description()
            );
            if let Some(path) = cookie.location() {
                println!("  location: {}", path.display());
            }
        }
        None => println!("session cookie: none"),
    }

    println!("config dir: {}", credentials::config_dir().display());
    Ok(())
}

fn logout(token_only: bool, session_only: bool) -> Result<()> {
    let (remove_token, remove_session) = match (token_only, session_only) {
        (false, false) => (true, true),
        (true, false) => (true, false),
        (false, true) => (false, true),
        (true, true) => (true, true),
    };

    if remove_token {
        let path = credentials::token_path();
        if credentials::remove_secret(&path)? {
            println!("Removed {}", path.display());
        } else {
            println!("No stored token at {}", path.display());
        }
    }

    if remove_session {
        let path = credentials::session_path();
        if credentials::remove_secret(&path)? {
            println!("Removed {}", path.display());
        } else {
            println!("No stored session at {}", path.display());
        }
    }

    println!("Environment variables were not changed.");
    Ok(())
}

fn prompt_secret(prompt: &str) -> Result<String> {
    let value = rpassword::prompt_password(prompt).context("reading hidden input")?;
    let value = value.trim().to_string();
    if value.is_empty() {
        bail!("no value entered");
    }
    Ok(value)
}

#[cfg(target_os = "macos")]
fn open_browser(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

#[cfg(target_os = "linux")]
fn open_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[cfg(target_os = "windows")]
fn open_browser(url: &str) {
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", url])
        .spawn();
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn open_browser(_url: &str) {}
