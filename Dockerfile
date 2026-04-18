# ── Stage 1: Build frontend (TypeScript client + Vite frontend) ───────────────
FROM node:22-alpine AS frontend-builder
WORKDIR /build
# Install and build the generated TS client package
COPY todo-typescript-client/package.json todo-typescript-client/package-lock.json ./todo-typescript-client/
RUN cd todo-typescript-client && npm ci
COPY todo-typescript-client/src/ ./todo-typescript-client/src/
COPY todo-typescript-client/tsconfig*.json ./todo-typescript-client/
RUN cd todo-typescript-client && npm run build
# Install and build the frontend (file: dep resolves to ../todo-typescript-client)
COPY frontend/package.json frontend/package-lock.json ./frontend/
RUN cd frontend && npm ci
COPY frontend/src/ ./frontend/src/
COPY frontend/index.html frontend/tsconfig.json ./frontend/
RUN cd frontend && npm run build

# ── Stage 2: Build the Rust binary ───────────────────────────────────────────
FROM rust:1-slim-bookworm AS rust-builder
RUN apt-get update && apt-get install -y libsqlite3-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY todo-server-sdk/ ./todo-server-sdk/
COPY smithy-rs/rust-runtime/ ./smithy-rs/rust-runtime/
# Dummy main to cache dependency compilation
RUN mkdir -p src && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src
COPY src/ ./src/
RUN touch src/main.rs && cargo build --release

# ── Stage 3: Minimal runtime image ───────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libsqlite3-0 ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-builder /build/target/release/todo ./
COPY --from=frontend-builder /build/frontend/dist/ ./frontend/dist/
VOLUME ["/data"]
ENV DATABASE_URL=sqlite:///data/todo.db?mode=rwc
EXPOSE 3000
CMD ["./todo"]
