# cratesdotio-mcp

MCP server for interacting with [crates.io](https://crates.io).

This mcp tool can be useful to define rules on your AI agent in order to:
- Use always the latest version of a crate
- Retrieve the github repository of a crate
- Access brief description of what a crate does
- Access dependencies of a crate

## Tools

- `search_crates` - Search crates by name/keyword
- `get_crate_info` - Get crate details
- `get_crate_versions` - List published versions
- `get_crate_dependencies` - Show dependencies for a version
- `get_crate_documentation` - Get docs.rs links

## Installation

```bash
cargo build --release
```

## Running

### Stdio Mode (default)
```bash
./target/release/cratesdotio-mcp
```

### HTTP Mode
```bash
# Default port 3000
./target/release/cratesdotio-mcp --http

# Custom port
./target/release/cratesdotio-mcp --http 8080
```

### Docker
```bash
docker build -t cratesdotio-mcp .

# HTTP mode
docker run -p 3000:3000 cratesdotio-mcp --http

# HTTP mode (custom port)
docker run -p 8080:8080 cratesdotio-mcp --http 8080

# Stdio mode
docker run -i cratesdotio-mcp
```

## MCP Configuration

Make sure that the mcp server is running **BEFORE** starting/configuring your AI agent in the editor: this will make sure that the initial session handshake is successful.

### HTTP (local or Docker)
```json
{
  "mcpServers": {
    "crates-io": {
      "serverUrl": "http://localhost:3000/sse"
    }
  }
}
```

### Stdio (local binary)
```json
{
  "mcpServers": {
    "crates-io": {
      "command": "/path/to/cratesdotio-mcp"
    }
  }
}
```


