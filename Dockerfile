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

# cargo-chef splits the Rust build in two: `cook` compiles only the dependencies, from a
# recipe made of the manifests and Cargo.lock, so that layer is reused until they change.
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY backend backend
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends libssl-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*
# build.rs would run npm; the frontend is built in the stage above.
ENV SKIP_FRONTEND_BUILD=1
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY backend backend
COPY --from=frontend /src/frontend/dist frontend/dist
RUN cargo build --release --locked -p budget-backend

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/budget-backend /usr/local/bin/

RUN groupadd --gid 10001 app \
 && useradd --uid 10001 --gid 10001 --no-create-home app \
 && mkdir /app
ENV BIND_ADDR=0.0.0.0:3000
USER app
WORKDIR /app
EXPOSE 3000
CMD ["budget-backend"]
