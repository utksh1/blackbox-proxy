# Build Stage
FROM rust:slim-bookworm as builder

# Install dependencies required for compilation (e.g. for rusqlite / libsqlite3 / aws-lc-rs)
RUN apt-get update && apt-get install -y pkg-config libssl-dev build-essential sqlite3 libsqlite3-dev cmake clang && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/blackbox-proxy

# Create a dummy src so we can cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && \
    echo "fn main() {println!(\"if you see this, the build failed\")}" > src/main.rs && \
    cargo build --release -j 1 && \
    rm -rf src

# Now copy the actual source code
COPY src ./src
# Touch main.rs to ensure Cargo detects the change and rebuilds the final binary
RUN touch src/main.rs

# Build the real application
RUN cargo build --release -j 1

# Runtime Stage
FROM debian:bookworm-slim

# Install runtime dependencies (OpenSSL, CA certificates, sqlite3)
RUN apt-get update && apt-get install -y ca-certificates libssl3 libsqlite3-0 && rm -rf /var/lib/apt/lists/*

# Copy the compiled binary from the builder stage
COPY --from=builder /usr/src/blackbox-proxy/target/release/blackbox-proxy /usr/local/bin/blackbox-proxy

# Expose the application port
EXPOSE 8080
ENV PORT=8080
ENV RUST_LOG=info

# Start the application
CMD ["blackbox-proxy"]
