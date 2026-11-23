#!/usr/bin/env bash
# enable-docker-nopass.sh — let you run `docker ...` with no sudo and no password.
#
# What it does:
#   1. Creates the `docker` group if missing
#   2. Adds your user to the `docker` group (standard Docker Linux post-install step)
#   3. Enables + starts the Docker daemon
#   4. Fixes ownership on /var/run/docker.sock if needed
#   5. Verifies with `docker ps`
#
# Usage:
#   chmod +x enable-docker-nopass.sh
#   ./enable-docker-nopass.sh
#
# After it runs you MUST refresh your groups once (pick one):
#   newgrp docker     # instant, only affects that shell
#   # or log out and back in
#
# SECURITY NOTE: members of the `docker` group are effectively root
# (they can mount any path via `docker run -v /:/host ...`). Only do this
# on a machine where you'd trust the user with sudo.

set -euo pipefail

# User to add — works whether you run ./script.sh or sudo ./script.sh
TARGET_USER="${SUDO_USER:-${USER:-$(id -un)}}"
if [[ "$TARGET_USER" == "root" ]]; then
  echo "ERROR: detected TARGET_USER=root. Run as your normal user, not 'sudo su' as root." >&2
  echo "Re-run as: ./enable-docker-nopass.sh" >&2
  exit 1
fi

echo "==> Target user: $TARGET_USER"

# 1. Docker must be installed
if ! command -v docker >/dev/null 2>&1; then
  echo "ERROR: 'docker' command not found. Install Docker Engine first:" >&2
  echo "  https://docs.docker.com/engine/install/ubuntu/" >&2
  exit 1
fi
echo "==> Found: $(docker --version)"

# 2. Create docker group if it doesn't exist
if ! getent group docker >/dev/null; then
  echo "==> Creating 'docker' group..."
  sudo groupadd docker
else
  echo "==> Group 'docker' already exists."
fi

# 3. Add user to docker group
if id -nG "$TARGET_USER" | tr ' ' '\n' | grep -qx docker; then
  echo "==> $TARGET_USER is already in the 'docker' group."
else
  echo "==> Adding $TARGET_USER to 'docker' group (will prompt for sudo password once)..."
  sudo usermod -aG docker "$TARGET_USER"
  echo "==> Added."
fi

# 4. Enable + start daemon (systemd systems; harmless no-op otherwise)
if command -v systemctl >/dev/null 2>&1; then
  echo "==> Enabling + starting Docker daemon..."
  sudo systemctl enable --now docker.socket docker.service >/dev/null 2>&1 || \
    sudo systemctl enable --now docker >/dev/null 2>&1 || true
fi

# 5. Fix socket ownership if it exists and is wrong (e.g. root:root after a rootful start)
SOCK="/var/run/docker.sock"
if [[ -S "$SOCK" ]]; then
  echo "==> Socket: $(ls -l "$SOCK")"
  if [[ "$(stat -c %G "$SOCK")" != "docker" ]]; then
    echo "==> Fixing socket group to 'docker'..."
    sudo chgrp docker "$SOCK" || true
    sudo chmod 660 "$SOCK" || true
  fi
else
  echo "==> No $SOCK yet (daemon may use a different socket or not be started). Skipping chgrp."
fi

echo ""
echo "==> Verifying access (with group applied via sg)..."
if sg docker -c "docker ps" >/dev/null 2>&1; then
  sg docker -c "docker ps" | head -5
  echo ""
  echo "SUCCESS: '$TARGET_USER' can run docker without a password."
else
  echo "NOTE: direct 'docker ps' failed in this shell (expected until you refresh groups)."
  echo "Trying with freshly-applied group..."
  if sg docker -c "docker ps"; then
    echo ""
    echo "SUCCESS via 'sg docker'. Your groups are fixed — just refresh this shell:"
  else
    echo ""
    echo "FAILED: even with 'docker' group, daemon unreachable. Try:" >&2
    echo "  sudo systemctl status docker" >&2
    echo "  sudo journalctl -u docker --no-pager -e" >&2
    exit 1
  fi
fi

echo ""
echo "Next step — refresh groups (pick ONE):"
echo "  newgrp docker        # instant fix for the current shell, then try: docker ps"
echo "  # or log out and log back in, then try: docker ps"
echo ""
echo "Test with: docker run --rm hello-world"
