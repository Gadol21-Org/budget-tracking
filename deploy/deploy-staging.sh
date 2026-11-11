#!/usr/bin/env bash
# Runs on the VM: replaces the staging budget tracker with the given image.
#   budget-staging  published on 0.0.0.0:8083 (production is 127.0.0.1:8082)
# Staging is fully isolated: its own app container, its own MySQL container on a private
# Docker network (data in the volume budget-staging-data, nothing published), and NO
# production settings. The app is started without --env-file, so there is no production
# MySQL URL, no Google Sheets and no Paamonim credentials, and nothing can reach those.
# Usage: deploy-staging.sh <image> [reset|teardown]
#   reset     wipes the data and reseeds it
#   teardown  removes staging entirely (containers, network, data volume, staging images)
#             and deploys nothing; safe to run when staging is already down
set -euo pipefail

image="${1:-}"
reset="${2:-}"
name=budget-staging
volume=budget-staging-data
host_port=8083
network=budget-staging-net
db=budget-staging-mysql

if [ "$reset" = "teardown" ]; then
  # Everything below is addressed by the exact staging names (or the staging-* image tag
  # prefix); nothing is selected by a wildcard that could match production
  # (budget-tracking, :latest images, its data). Each step tolerates "already gone".
  docker rm -f "$name" "$db" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  docker volume rm "$volume" >/dev/null 2>&1 || true
  # Staging images only: tags starting with "staging-" (production uses other tags).
  docker images --format '{{.Repository}}:{{.Tag}}' --filter 'reference=*:staging-*' \
    | grep -E ':staging-[^:]+$' \
    | xargs -r docker rmi >/dev/null 2>&1 || true
  # The sample-database image is removed only if nothing else (e.g. another container)
  # still uses it; docker refuses otherwise.
  docker rmi mysql:8.4 >/dev/null 2>&1 || true
  echo "Staging torn down."
  exit 0
fi

docker pull "$image"
docker pull mysql:8.4
docker rm -f "$name" >/dev/null 2>&1 || true
if [ "$reset" = "reset" ]; then
  docker rm -f "$db" >/dev/null 2>&1 || true
  docker volume rm "$volume" >/dev/null 2>&1 || true
fi

# The sample database: reachable only from the staging app, over its own network.
docker network inspect "$network" >/dev/null 2>&1 || docker network create "$network" >/dev/null
if [ "$(docker inspect -f '{{.State.Running}}' "$db" 2>/dev/null)" != "true" ]; then
  docker rm -f "$db" >/dev/null 2>&1 || true
  docker run -d --name "$db" \
    --restart unless-stopped \
    --network "$network" \
    -v "$volume":/var/lib/mysql \
    -e MYSQL_ROOT_PASSWORD=staging \
    mysql:8.4 --innodb-buffer-pool-size=64M
fi
for _ in $(seq 1 60); do
  docker exec "$db" mysqladmin ping -h 127.0.0.1 -pstaging --silent >/dev/null 2>&1 && break
  sleep 2
done

# Optional staging-only settings (Sumit test account, VAPID push keys, email), written by the workflow
# from staging secrets. They are never the production ones.
extra_env=()
[ -f "$HOME/budget-staging/staging.env" ] && extra_env=(--env-file "$HOME/budget-staging/staging.env")

docker run -d --name "$name" \
  --init \
  --restart unless-stopped \
  --log-opt max-size=10m --log-opt max-file=3 \
  -p "0.0.0.0:$host_port:3000" \
  --network "$network" \
  -e MYSQL_URL="mysql://root:staging@$db:3306/budget" \
  -e PAAMONIM_MIRROR=0 \
  "${extra_env[@]}" \
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
