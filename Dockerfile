# syntax=docker/dockerfile:1
# The budget tracker: Rust backend with the PWA embedded.
# Built natively on the target architecture (an arm64 runner for an arm64 Oracle VM),
# so there is no cross-compiling.

FROM node:22-bookworm-slim AS frontend
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends libssl-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# build.rs would run npm; the frontend is built in the stage above.
ENV SKIP_FRONTEND_BUILD=1

# Build the dependencies against stub sources first, so this layer is reused
# until Cargo.toml or Cargo.lock change.
COPY Cargo.toml Cargo.lock ./
COPY backend/Cargo.toml backend/Cargo.toml
RUN mkdir backend/src \
 && echo 'fn main() {}' > backend/src/main.rs \
 && echo 'fn main() {}' > backend/build.rs \
 && cargo build --release --locked -p budget-backend \
 && rm -rf backend/src backend/build.rs

COPY backend backend
COPY --from=frontend /src/frontend/dist frontend/dist
# Cargo compares mtimes; make sure the real sources count as newer than the stubs.
RUN touch backend/src/main.rs backend/build.rs \
 && cargo build --release --locked -p budget-backend

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/budget-backend /usr/local/bin/

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
