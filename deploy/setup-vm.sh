#!/usr/bin/env bash
# One-time setup of an Ubuntu VM for deploys: installs Docker and lets this user
# run it. Each project then keeps its files in its own folder under this user's
# home, such as ~/budget-tracking.
set -euo pipefail

if ! command -v docker >/dev/null; then
  curl -fsSL https://get.docker.com | sudo sh
fi
sudo usermod -aG docker "$USER"
echo "Done. Log out and back in so docker works without sudo."
