# cratesdotio-mcp

MCP server for [crates.io](https://crates.io). It reads crate data by default. With an explicit mode it can also scaffold a crate, publish it, yank versions, manage owners, set up trusted publishing for GitHub Actions, and, with a session cookie, manage account-level operations such as API tokens and crate deletion.

## Modes

The server refuses to start without `--mode` or `CRATES_MCP_MODE`. There is no default, so a server that can publish cannot start by accident.

| Mode | What it adds | Credential |
| --- | --- | --- |
| `read-only` | Search, crate info, versions, dependencies, docs, name availability | None |
| `write` | Owner management, yank and unyank, trusted publishing configuration | API token |
| `publish` | Local scaffolding, workflow generation, `cargo publish` | API token |
| `admin` | Whoami, token listing, creation and revocation, crate deletion | Session cookie, plus a token for write and publish tools |

Write, publish, and admin tools are not listed in a lower mode. Calling one anyway returns an error that names the required mode.

## Tools

Read-only, available in every mode:

| Tool | Purpose |
| --- | --- |
| `search_crates` | Search by name or keyword |
| `get_crate_info` | Crate metadata and latest version |
| `get_crate_versions` | Published versions and yank state |
| `get_crate_dependencies` | Dependencies of a version |
| `get_crate_documentation` | docs.rs links |
| `check_crate_name` | Validate syntax and check whether the name is taken |
| `crates_auth_status` | Report mode, token and cookie presence, and their sources. Never prints values |

`write` mode:

| Tool | Token scope |
| --- | --- |
| `list_crate_owners` | none, owner data is public |
| `add_crate_owner` | `change-owners` |
| `remove_crate_owner` | `change-owners` |
| `yank_crate_version` | `yank` |
| `unyank_crate_version` | `yank` |
| `list_trustpub_github_configs` | `trusted-publishing` |
| `create_trustpub_github_config` | `trusted-publishing` |
| `delete_trustpub_github_config` | `trusted-publishing` |
| `set_trustpub_only` | `trusted-publishing` |

`publish` mode:

| Tool | Purpose |
| --- | --- |
| `scaffold_crate` | `cargo new` plus a publishable `Cargo.toml` and README |
| `publish_crate` | `cargo publish`, with a dry run by default |
| `generate_publish_workflow` | Write `.github/workflows/<name>.yml` using trusted publishing |

`admin` mode, on top of everything above when a token is also present:

| Tool | Purpose |
| --- | --- |
| `crates_whoami` | Account login, email verification state, owned crates |
| `list_api_tokens` | Active tokens with scopes, creation and last-used times |
| `create_api_token` | Create a scoped token. Stores it by default instead of returning it |
| `revoke_api_token` | Revoke a token by id, confirmed with `revoke <id>` |
| `delete_crate` | Permanently delete a crate, confirmed with `delete <name>` |

## Authentication

Create a token at <https://crates.io/settings/tokens> with only the scopes you need:

| Scope | Allows |
| --- | --- |
| `publish-new` | Publish a crate for the first time |
| `publish-update` | Publish new versions of an existing crate |
| `yank` | Yank and unyank versions |
| `change-owners` | Add and remove owners |
| `trusted-publishing` | Manage trusted publishing configs and the trustpub-only flag |

The server reads the token from the first source that provides one:

1. `CRATES_IO_TOKEN`
2. `CARGO_REGISTRY_TOKEN`
3. `$CRATES_MCP_CONFIG_DIR/token`, written by `cratesdotio-mcp login` or `mint-token`
4. `$CARGO_HOME/credentials.toml`, `registries.crates-io.token` or the legacy `registry.token`

A token is never accepted as a tool argument, because tool arguments end up in model context and transcripts. The value is redacted from debug output and never logged.

The command line stores credentials for you:

```bash
# Paste a token once, with hidden input
cratesdotio-mcp login

# Or create a scoped token from a stored session cookie
cratesdotio-mcp mint-token --name laptop-mcp --scopes publish-update,yank

# Show which credentials exist, without printing values
cratesdotio-mcp status

# Remove stored credentials
cratesdotio-mcp logout
```

`mint-token` stores the new token at `$CRATES_MCP_CONFIG_DIR/token` and does not print it unless `--reveal` is passed. Use `--reveal` when you need the value for a CI secret.

### Session cookie for admin mode

`admin` mode needs the `cargo_session` cookie:

```bash
cratesdotio-mcp session
```

The command opens the token page, explains where to find the cookie in browser devtools, reads the value with hidden input, verifies it with a whoami request, and stores it at `$CRATES_MCP_CONFIG_DIR/session` with mode 0600.

The cookie grants full account authority: token creation, crate deletion, email and account settings. It is only loaded in `admin` mode, never accepted as a tool argument, redacted from output, and never sent over plaintext HTTP. `admin` mode refuses a non-loopback HTTP bind. The tool never handles your GitHub password and never automates the GitHub login or 2FA.

### What is still impossible

- **Automating the GitHub login.** crates.io signs in through GitHub OAuth. A scripted login would need your GitHub password and 2FA, and the resulting session would be the same full-authority credential the cookie already is. Import a real browser session instead.
- **Mapping a token value to its scopes.** crates.io returns a token value only once, at creation, and never exposes it again. `list_api_tokens` shows every token's scopes, but cannot tell you which entry belongs to the value in `CRATES_IO_TOKEN`. A missing scope still shows up as HTTP 403 on the first operation.
- **Creating a crate without publishing.** There is no create-crate endpoint. A crate exists after its first version is uploaded, so trusted publishing can only be configured after that first publish.

## Cargo-side notes

- Workspace-inherited package fields are resolved through `cargo metadata`, so members that use `[workspace.package]` publish normally.
- The publish tool always targets the `crates-io` registry, even when the user's cargo config names a different default registry.

## Flows

### New crate

```
scaffold_crate { name, parent_dir, description, license, repository }
```

Then add code and dependencies and commit the new files, because cargo refuses to publish a dirty git tree unless `allow_dirty` is set. Run:

```
publish_crate { directory, dry_run: true }
```

The dry run packages the crate and runs the verification build. When it passes:

```
publish_crate { directory, dry_run: false, confirm: "name@0.1.0" }
```

A real publish requires `confirm` to equal `name@version` exactly. Publishing is irreversible after 72 hours, and a version number can never be reused.

### CI with trusted publishing

Trusted publishing lets GitHub Actions publish with a short-lived OIDC token, so no long-lived secret lives in the repository.

The crate must exist before a trusted publishing config can be created, so the order for a new crate is:

1. Publish the first version with a scoped token.
2. `generate_publish_workflow { directory, name, filename }`
3. Commit and push the workflow to the default branch.
4. `create_trustpub_github_config { name, repository_owner, repository_name, workflow_filename }`
5. Optionally `set_trustpub_only { name, trustpub_only: true }` to stop long-lived tokens from publishing new versions.
6. Tag a release. The workflow calls `rust-lang/crates-io-auth-action@v1`, gets a short-lived token, and runs `cargo publish`.

The `workflow_filename` in the config must match the file in `.github/workflows`. Creating the config also requires the crates.io account to have a linked GitHub account and a verified email, and the user must be a crate owner.

### Existing crate

```
publish_crate { directory, dry_run: true }
publish_crate { directory, dry_run: false, confirm: "name@version" }
```

### Yank, owners, trustpub

Yank and unyank are reversible and need no confirmation. Owner changes may create invitations that the invitee must accept. `set_trustpub_only` rejects publishes from long-lived tokens but does not revoke tokens that already exist.

### Account operations and deletion

In `admin` mode, with a stored session cookie:

```
crates_whoami
list_api_tokens
create_api_token { name: "laptop-mcp", endpoint_scopes: ["publish-update", "yank"] }
delete_crate { name: "old-crate", confirm: "delete old-crate" }
```

`create_api_token` writes the new value to the token file and reports only the id and path, so the secret stays out of the transcript. Set `reveal: true` only when you need the value in the conversation. The running server keeps the token it loaded at startup, so restart it after creating a new one.

`delete_crate` is permanent. crates.io only allows it within 72 hours of creation, or for a single-owner crate with under 1000 downloads per month, no reverse dependencies, and no team owners. The name stays reserved for 24 hours.

## Running

```bash
cargo build --release

# stdio, read-only
./target/release/cratesdotio-mcp --mode read-only

# stdio, publish
CRATES_IO_TOKEN=... ./target/release/cratesdotio-mcp --mode publish

# stdio, admin (session cookie, token optional)
cratesdotio-mcp --mode admin

# HTTP, read-only, port 3000
./target/release/cratesdotio-mcp --mode read-only --http
```

Environment variables:

| Variable | Purpose |
| --- | --- |
| `CRATES_MCP_MODE` | Same as `--mode` |
| `CRATES_IO_TOKEN` | Token, highest precedence |
| `CARGO_REGISTRY_TOKEN` | Token, second precedence |
| `CRATES_IO_COOKIE` | `cargo_session` cookie value for admin mode, highest precedence |
| `CRATES_IO_COOKIE_FILE` | File containing the `cargo_session` value |
| `CRATES_MCP_CONFIG_DIR` | Credential directory, default `$XDG_CONFIG_HOME/cratesdotio-mcp` or `~/.config/cratesdotio-mcp` |
| `CRATES_MCP_BASE_URL` | Base URL for authenticated API calls, default `https://crates.io`. The read client always uses crates.io. Token calls allow loopback http for tests; the session-cookie client always requires https. Credentials are sent to this host |
| `CARGO` | Path to the cargo binary, used for scaffolding and publishing |

### MCP configuration

stdio:

```json
{
  "mcpServers": {
    "crates-io": {
      "command": "/path/to/cratesdotio-mcp",
      "args": ["--mode", "publish"],
      "env": { "CRATES_IO_TOKEN": "..." }
    }
  }
}
```

HTTP:

```json
{
  "mcpServers": {
    "crates-io": {
      "serverUrl": "http://localhost:3000/sse"
    }
  }
}
```

### Docker

The image runs read-only HTTP:

```bash
docker build -t cratesdotio-mcp .
docker run -d -p 8080:8080 cratesdotio-mcp --http 8080
```

Publishing needs the crate on the same filesystem as the server and a local cargo, so publish and admin modes are stdio-only. A server in any mode other than read-only refuses to bind to a non-loopback address over HTTP.

## Security notes

- `cargo publish` runs the crate's build scripts and proc macros. Publish mode is local code execution. Do not point it at untrusted crates.
- Write, publish, and admin modes hold real credentials. A token is required for write and publish; a session cookie is required for admin. Every mode above read-only refuses a non-loopback HTTP bind.
- `admin` mode holds a session cookie with full account authority. It is stored with mode 0600, loaded only in admin mode, and never returned to the model. Prefer a scoped token for anything that does not need account operations.
- Publishing is permanent after 72 hours. The `confirm` argument exists to stop accidental publishes, including from an agent that guessed the next version. Crate deletion is permanent and requires `confirm` to name the crate.
- Credentials are read once at startup and passed to cargo through the environment, not through the command line. Cargo output and API responses are scrubbed of credentials before they are returned, including on build failures.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

The unit and integration tests cover token and cookie precedence and redaction, mode gating, crate name and version validation, manifest rendering, publish preflight rules, workflow rendering, and the CLI gates. They do not contact crates.io.
