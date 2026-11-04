FROM rust:bookworm AS builder

WORKDIR /usr/src/convena-api

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs
RUN cargo build --release || true
RUN rm -rf src

# Copy real source files
COPY src/ ./src/

# Compile production binary
RUN touch src/main.rs src/lib.rs && cargo build --release

# Stage 2: Minimal runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/src/convena-api/target/release/convena-api /app/convena-api

EXPOSE 8000
ENV PORT=8000

CMD ["/app/convena-api"]
