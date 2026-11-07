# cratesdotio-mcp

MCP server for [crates.io](https://crates.io). It reads crate data by default. With an explicit mode it can also scaffold a crate, publish it, yank versions, manage owners, and set up trusted publishing for GitHub Actions.

## Modes

The server refuses to start without `--mode` or `CRATES_MCP_MODE`. There is no default, so a server that can publish cannot start by accident.

| Mode | What it adds | Token |
| --- | --- | --- |
| `read-only` | Search, crate info, versions, dependencies, docs, name availability | Not read |
| `write` | Owner management, yank and unyank, trusted publishing configuration | Required |
| `publish` | Local scaffolding, workflow generation, `cargo publish` | Required |

Write and publish tools are not listed in a lower mode. Calling one anyway returns an error that names the required mode.

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
| `crates_auth_status` | Report mode, token presence, and token source. Never prints the token |

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
3. `$CARGO_HOME/credentials.toml`, `registries.crates-io.token` or the legacy `registry.token`

A token is never accepted as a tool argument, because tool arguments end up in model context and transcripts. The value is redacted from debug output and never logged.

### What the server cannot do

These are crates.io limits, not missing features:

- **Create an API token.** `PUT /api/v1/me/tokens` is session-cookie only and returns `400 cannot use an API token to create a new API token` when called with a token. There is no supported way for a tool holding a token to mint another token.
- **Log in as you.** Session login needs the web UI, and the session cookie gives full account access. This server never asks for or stores cookies.
- **Verify token scopes.** crates.io has no token introspection endpoint (`/api/v1/me` is cookie-only). A missing scope appears as HTTP 403 on the first operation that needs it.
- **Delete a crate.** `DELETE /api/v1/crates/{name}` is cookie-only and also limited by crate age, downloads, and reverse dependencies.

Create the token in the browser once, then give it to the server through the environment.

## Cargo-side limits

- Crates that inherit `version`, `description`, `license`, `license-file`, `repository`, or `publish` from `[workspace.package]` are rejected before publishing, because the confirm string needs literal values. Run `cargo publish` directly for those crates.
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

## Running

```bash
cargo build --release

# stdio, read-only
./target/release/cratesdotio-mcp --mode read-only

# stdio, publish
CRATES_IO_TOKEN=... ./target/release/cratesdotio-mcp --mode publish

# HTTP, read-only, port 3000
./target/release/cratesdotio-mcp --mode read-only --http
```

Environment variables:

| Variable | Purpose |
| --- | --- |
| `CRATES_MCP_MODE` | Same as `--mode` |
| `CRATES_IO_TOKEN` | Token, highest precedence |
| `CARGO_REGISTRY_TOKEN` | Token, second precedence |
| `CRATES_MCP_BASE_URL` | Base URL for write API calls, default `https://crates.io`. The read client always uses crates.io. It must use https, except for loopback test servers. The token is sent to this host |
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

Publishing needs the crate on the same filesystem as the server and a local cargo, so write and publish modes are stdio-only. A write-mode server refuses to bind to a non-loopback address over HTTP.

## Security notes

- `cargo publish` runs the crate's build scripts and proc macros. Publish mode is local code execution. Do not point it at untrusted crates.
- Write and publish modes hold a real credential. They refuse to start without a token, and they refuse a non-loopback HTTP bind.
- Publishing is permanent after 72 hours. The `confirm` argument exists to stop accidental publishes, including from an agent that guessed the next version.
- Tokens are read once at startup and passed to cargo through the environment, not through the command line. Cargo output is scrubbed of the token before it is returned, including on build failures.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

The unit tests cover token precedence and redaction, mode gating, crate name and version validation, manifest rendering, publish preflight rules, and workflow rendering. They do not contact crates.io.
