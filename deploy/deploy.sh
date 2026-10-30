#!/usr/bin/env bash
# Runs on the VM: replaces the running budget tracker with the given image.
#   budget-tracking  published on 127.0.0.1:8082 of the VM only
# The cloudflared service on the VM forwards a public hostname to that port.
# It is not part of the deploy and keeps running.
# Usage: deploy.sh <image>
set -euo pipefail

image="$1"
dir="$HOME/budget-tracking"
env_file="$dir/.env"
name=budget-tracking
host_port=8082

if [ ! -f "$env_file" ]; then
  echo "$env_file is missing; the workflow writes it from the BUDGET_ENV secret" >&2
  exit 1
fi

# Pull before stopping anything, so the app is down only for the swap.
docker pull "$image"

# Bound to the loopback address, so it is reachable from the VM only, not from the internet.
docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" \
  --init \
  --restart unless-stopped \
  --log-opt max-size=10m --log-opt max-file=3 \
  -p "127.0.0.1:$host_port:3000" \
  -v budget-data:/data \
  --env-file "$env_file" \
  "$image"

# An app that can't start (bad NoSQL settings, say) exits at once and restarts
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
