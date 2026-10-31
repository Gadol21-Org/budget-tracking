#!/usr/bin/env bash
# Runs on the VM: replaces the staging budget tracker with the given image.
#   budget-staging  published on 0.0.0.0:8083 (production is 127.0.0.1:8082)
# Staging is fully isolated: its own container, its own Docker volume (budget-staging-data,
# SQLite), and NO production settings. It is started without --env-file, so there is no
# MySQL URL, no Google Sheets and no Paamonim credentials, and nothing can reach those.
# Usage: deploy-staging.sh <image> [reset]     "reset" wipes the data and reseeds it
set -euo pipefail

image="$1"
reset="${2:-}"
name=budget-staging
volume=budget-staging-data
host_port=8083

docker pull "$image"
docker rm -f "$name" >/dev/null 2>&1 || true
if [ "$reset" = "reset" ]; then
  docker volume rm "$volume" >/dev/null 2>&1 || true
fi

docker run -d --name "$name" \
  --init \
  --restart unless-stopped \
  --log-opt max-size=10m --log-opt max-file=3 \
  -p "0.0.0.0:$host_port:3000" \
  -v "$volume":/data \
  -e STORAGE=sqlite \
  -e DATABASE_URL=/data/budget.db \
  -e PAAMONIM_MIRROR=0 \
  "$image"

for _ in $(seq 1 30); do
  curl -fsS "http://127.0.0.1:$host_port/api/categories" >/dev/null 2>&1 && break
  sleep 2
done
if [ "$(docker inspect -f '{{.State.Running}} {{.RestartCount}}' "$name")" != "true 0" ]; then
  echo "$name did not stay up:" >&2
  docker logs --tail 50 "$name" >&2
  exit 1
fi

bash "$HOME/budget-staging/seed-staging.sh" "http://127.0.0.1:$host_port"
docker image prune -f >/dev/null
