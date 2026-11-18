#!/usr/bin/env bash
# Runs on the VM: replaces the running budget tracker with the given image.
#   budget-tracking  published on 127.0.0.1:8082 of the VM only, and on the Docker network
#                    budget-internal, where other containers reach it as http://budget-tracking:3000
#                    (including its /mcp endpoint, which has no login: see deploy/MCP.md)
# The cloudflared service on the VM forwards a public hostname to that port.
# It is not part of the deploy and keeps running.
# Usage: deploy.sh <image>
set -euo pipefail

image="$1"
dir="$HOME/budget-tracking"
env_file="$dir/.env"
name=budget-tracking
host_port=8082
network=budget-internal

if [ ! -f "$env_file" ]; then
  echo "$env_file is missing; the workflow writes it from the BUDGET_ENV secret" >&2
  exit 1
fi

# Pull before stopping anything, so the app is down only for the swap.
docker pull "$image"

# The private network other containers on this VM use to reach the app (made once).
docker network inspect "$network" >/dev/null 2>&1 || docker network create "$network" >/dev/null

# Bound to the loopback address, so it is reachable from the VM only, not from the internet.
docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" \
  --init \
  --restart unless-stopped \
  --log-opt max-size=10m --log-opt max-file=3 \
  -p "127.0.0.1:$host_port:3000" \
  --network "$network" \
  --env-file "$env_file" \
  "$image"

# An app that can't start (a bad MYSQL_URL, say) exits at once and restarts
# in a loop; catch that here.
sleep 20
if [ "$(docker inspect -f '{{.State.Running}} {{.RestartCount}}' "$name")" != "true 0" ]; then
  echo "$name did not stay up:" >&2
  docker logs --tail 50 "$name" >&2
  exit 1
fi
docker logs --tail 10 "$name"

# Not a failure: the app is up, but nothing can reach it from outside.
systemctl is-active --quiet cloudflared 2>/dev/null \
  || echo "warning: the cloudflared service is not running on this VM" >&2

docker image prune -f >/dev/null
