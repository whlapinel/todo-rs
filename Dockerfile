# ── Stage 1: Build Tailwind CSS for the web_ui templates ──────────────────────
FROM --platform=$BUILDPLATFORM node:22-alpine AS styles-builder
WORKDIR /build
COPY styles/package.json styles/package-lock.json ./styles/
RUN cd styles && npm ci
COPY styles/input.css ./styles/
COPY templates/ ./templates/
RUN cd styles && npm run build

# ── Stage 2: Build the Rust binary (cross-compile to x86_64) ─────────────────
# --platform=$BUILDPLATFORM: run this stage natively on the host (e.g. arm64 Mac)
# instead of through QEMU emulation. The compiler runs at full speed and outputs
# x86_64 machine code directly.
FROM --platform=$BUILDPLATFORM rust:1-slim-bookworm AS rust-builder

# gcc-x86-64-linux-gnu: the cross-linker. rustc can generate x86_64 code on arm64,
# but needs a linker that knows how to assemble it into an x86_64 ELF binary.
RUN apt-get update && apt-get install -y gcc-x86-64-linux-gnu && rm -rf /var/lib/apt/lists/*

# Add the x86_64 Rust target (the stdlib pre-compiled for that architecture).
RUN rustup target add x86_64-unknown-linux-gnu

# Tell cargo which linker and C compiler to use for the x86_64 target.
# CC_* is read by the cc crate, which compiles bundled C code (SQLite amalgamation).
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
ENV CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY todo-server-sdk/ ./todo-server-sdk/
COPY smithy-rs/rust-runtime/ ./smithy-rs/rust-runtime/
# Dummy main to cache dependency compilation
RUN mkdir -p src && echo "fn main() {}" > src/main.rs \
    && cargo build --release --target x86_64-unknown-linux-gnu \
    && rm -rf src
COPY src/ ./src/
COPY templates/ ./templates/
RUN touch src/main.rs && cargo build --release --target x86_64-unknown-linux-gnu

# ── Stage 3: Minimal runtime image ───────────────────────────────────────────
FROM --platform=linux/amd64 debian:bookworm-slim
# SQLite is statically linked (bundled feature in Cargo.toml), so libsqlite3-0 is not needed.
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-builder /build/target/x86_64-unknown-linux-gnu/release/todo ./
COPY --from=styles-builder /build/static/ ./static/
VOLUME ["/data"]
ENV TODO_DATABASE_URL=sqlite:///data/todo.db?mode=rwc
EXPOSE 3000
CMD ["./todo"]
