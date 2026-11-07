# Build stage
FROM rust:1.88-slim AS builder

WORKDIR /app

# Install dependencies for building
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Copy source code
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build release binary
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /app/target/release/cratesdotio-mcp /usr/local/bin/

# Run as non-root user
RUN useradd -m -s /bin/bash mcp
USER mcp

# Expose HTTP port (for --http mode)
EXPOSE 3000

# Runs read-only over HTTP. Write and publish modes refuse a non-loopback HTTP
# bind; real publishes use stdio so the crate and cargo share a filesystem.
ENTRYPOINT ["cratesdotio-mcp", "--mode", "read-only"]
