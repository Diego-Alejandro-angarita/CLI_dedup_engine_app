# Build Stage
FROM rust:1.80-slim-bookworm as builder

WORKDIR /usr/src/app
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release

# Runtime Stage
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Create a non-root user and setup home directory for testing local repo behavior
RUN useradd -m dedupuser
USER dedupuser
ENV HOME=/home/dedupuser
WORKDIR /home/dedupuser

COPY --from=builder /usr/src/app/target/release/dedup /usr/local/bin/dedup

ENTRYPOINT ["dedup"]
