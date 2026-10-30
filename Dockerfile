# syntax=docker/dockerfile:1
# The budget tracker: Rust backend with the PWA embedded.
# The frontend and the Rust build run on the build machine's own architecture and
# cross-compile when the target differs (an arm64 Oracle VM from an amd64 GitHub runner).

FROM --platform=$BUILDPLATFORM node:22-bookworm-slim AS frontend
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM --platform=$BUILDPLATFORM rust:1-bookworm AS build
ARG BUILDARCH
ARG TARGETARCH
WORKDIR /src
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-gnu > /triple; echo x86-64-linux-gnu > /gnu ;; \
      arm64) echo aarch64-unknown-linux-gnu > /triple; echo aarch64-linux-gnu > /gnu ;; \
      *) echo "unsupported architecture $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && rustup target add "$(cat /triple)" \
 && if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
      apt-get update && apt-get install -y --no-install-recommends "gcc-$(cat /gnu | tr _ -)" libc6-dev-"$TARGETARCH"-cross \
      && rm -rf /var/lib/apt/lists/*; \
    fi
COPY . .
COPY --from=frontend /src/frontend/dist frontend/dist
# build.rs would run npm; the frontend is already built above. OpenSSL is compiled
# from source (vendored-openssl) so cross-compiling needs no target libssl.
ENV SKIP_FRONTEND_BUILD=1
RUN triple="$(cat /triple)" \
 && if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
      gcc="$(cat /gnu)-gcc"; \
      env_triple="$(echo "$triple" | tr a-z- A-Z_)"; \
      export "CARGO_TARGET_${env_triple}_LINKER=$gcc" "CC_$(echo "$triple" | tr - _)=$gcc"; \
    fi \
 && cargo build --release --locked --features vendored-openssl --target "$triple" -p budget-backend \
 && mkdir /out && cp "target/$triple/release/budget-backend" /out/

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/ /usr/local/bin/

# The SQLite file (only used with STORAGE=sqlite) lives in /data, a Docker volume.
RUN groupadd --gid 10001 app \
 && useradd --uid 10001 --gid 10001 --home-dir /data --create-home app \
 && mkdir /app
ENV DATABASE_URL=/data/budget.db \
    BIND_ADDR=0.0.0.0:3000
USER app
VOLUME /data
WORKDIR /app
EXPOSE 3000
CMD ["budget-backend"]
