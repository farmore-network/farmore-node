#!/usr/bin/env bash
#
# Farmore — Oracle Cloud (Ampere A1 / ARM64 / aarch64, Ubuntu) setup.
#
# Builds the farmore-node + farmore-resolver release binaries NATIVELY on this ARM64 box
# (so they are aarch64 ELF by construction — no cross-compilation, no "exec format error"),
# installs them under /opt/farmore, drops config templates under /etc/farmore, installs the
# systemd units, and opens the resolver port in the INSTANCE firewall.
#
# Run as the normal Ubuntu user (NOT root); the script calls sudo where it needs to:
#   bash setup-oracle.sh
#
# It is idempotent — safe to re-run (e.g. after a code update) to rebuild and reinstall.
#
# This handles the INSTANCE-level firewall only. You MUST ALSO open the same port in the
# Oracle web console (Security List / NSG ingress) — that is a separate, [HUMAN] step.

set -euo pipefail

# ---- Tunables (override via env) ---------------------------------------------------------
GIT_BASE="${GIT_BASE:-https://github.com/farmore-network}"   # or git@github.com:farmore-network
BRANCH="${BRANCH:-main}"
SRC_DIR="${SRC_DIR:-$HOME/farmore/src}"                       # sibling checkouts live here
RESOLVER_PORT="${RESOLVER_PORT:-8080}"                        # must match RESOLVER_BIND + Oracle NSG
SERVICE_USER="${SERVICE_USER:-farmore}"
PREFIX="${PREFIX:-/opt/farmore}"
# The node/resolver workspace needs these siblings (path deps in Cargo.toml).
REPOS=(farmore-node farmore-core farmore-ethereum-adapter)
# ------------------------------------------------------------------------------------------

say() { printf '\n\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\n\033[1;33m[warn]\033[0m %s\n' "$*"; }

# ---- 0. Sanity: architecture -------------------------------------------------------------
ARCH="$(uname -m)"
say "Architecture: $ARCH"
if [ "$ARCH" != "aarch64" ] && [ "$ARCH" != "arm64" ]; then
  warn "This box is NOT aarch64. This script targets Oracle Ampere A1 (ARM64)."
  warn "A native build here will produce $ARCH binaries, which will NOT run on the A1."
  read -r -p "Continue anyway? [y/N] " ans; [ "${ans:-N}" = "y" ] || exit 1
fi

# ---- 1. System packages ------------------------------------------------------------------
say "Installing build + runtime dependencies (apt)"
sudo apt-get update -y
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
  build-essential pkg-config libssl-dev cmake clang \
  git curl ca-certificates

# ---- 2. Swap (Rust + alloy linking is memory-hungry; small free-tier boxes can OOM) ------
MEM_KB="$(grep MemTotal /proc/meminfo | awk '{print $2}')"
if [ "$MEM_KB" -lt 8000000 ] && [ ! -f /swapfile ] && ! swapon --show | grep -q .; then
  say "Low RAM ($((MEM_KB/1024)) MB) and no swap — creating a 4G swapfile to avoid OOM during the build"
  sudo fallocate -l 4G /swapfile || sudo dd if=/dev/zero of=/swapfile bs=1M count=4096
  sudo chmod 600 /swapfile
  sudo mkswap /swapfile
  sudo swapon /swapfile
  grep -q '/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab >/dev/null
fi

# ---- 3. Rust toolchain (rustup; the repo's rust-toolchain.toml pins 1.96.0) ---------------
if ! command -v cargo >/dev/null 2>&1; then
  say "Installing Rust via rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"
say "cargo: $(cargo --version)"

# ---- 4. Fetch the three sibling repos ----------------------------------------------------
say "Fetching source into $SRC_DIR (siblings, so the Cargo path deps resolve)"
mkdir -p "$SRC_DIR"
for repo in "${REPOS[@]}"; do
  if [ -d "$SRC_DIR/$repo/.git" ]; then
    say "update $repo"; git -C "$SRC_DIR/$repo" fetch --depth 1 origin "$BRANCH" && git -C "$SRC_DIR/$repo" checkout -f "origin/$BRANCH"
  else
    say "clone $repo"; git clone --depth 1 --branch "$BRANCH" "$GIT_BASE/$repo.git" "$SRC_DIR/$repo"
  fi
done

# ---- 5. Build the two release binaries (native aarch64) -----------------------------------
say "Building release binaries (this can take several minutes on a small A1)"
cd "$SRC_DIR/farmore-node"
# Only the two binaries — avoids needing farmore-contracts (which only the e2e tests use).
cargo build --release -p farmore-node -p farmore-resolver
file target/release/farmore-node target/release/farmore-resolver || true

# ---- 6. Service user + install layout ----------------------------------------------------
say "Creating service user '$SERVICE_USER' and install dirs"
id -u "$SERVICE_USER" >/dev/null 2>&1 || sudo useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
sudo mkdir -p "$PREFIX/bin" /etc/farmore

say "Installing binaries to $PREFIX/bin"
sudo install -m 0755 target/release/farmore-node     "$PREFIX/bin/farmore-node"
sudo install -m 0755 target/release/farmore-resolver "$PREFIX/bin/farmore-resolver"

# ---- 7. Config templates (never overwrite a filled-in file) ------------------------------
DEPLOY_DIR="$SRC_DIR/farmore-node/deploy"
if [ ! -f /etc/farmore/node.env ]; then
  say "Installing /etc/farmore/node.env (FILL IN addresses + operator key, then chmod is already 600)"
  sudo install -m 0600 -o root -g root "$DEPLOY_DIR/farmore-node.env.example" /etc/farmore/node.env
else
  warn "/etc/farmore/node.env already exists — left untouched."
fi
if [ ! -f /etc/farmore/resolver.env ]; then
  say "Installing /etc/farmore/resolver.env (FILL IN addresses)"
  sudo install -m 0644 -o root -g root "$DEPLOY_DIR/farmore-resolver.env.example" /etc/farmore/resolver.env
else
  warn "/etc/farmore/resolver.env already exists — left untouched."
fi

# ---- 8. systemd units --------------------------------------------------------------------
say "Installing systemd units"
sudo install -m 0644 "$DEPLOY_DIR/farmore-resolver.service" /etc/systemd/system/farmore-resolver.service
sudo install -m 0644 "$DEPLOY_DIR/farmore-node.service"     /etc/systemd/system/farmore-node.service
sudo systemctl daemon-reload

# ---- 9. Instance firewall (resolver port) ------------------------------------------------
# The node makes only OUTBOUND RPC calls and listens on no port — only the resolver needs an
# ingress rule. Oracle Ubuntu ships iptables rules that DROP/REJECT inbound by default, so we
# INSERT an ACCEPT for the resolver port ahead of that REJECT, then persist it.
say "Opening TCP $RESOLVER_PORT in the instance firewall (iptables)"
if ! sudo iptables -C INPUT -p tcp --dport "$RESOLVER_PORT" -m conntrack --ctstate NEW -j ACCEPT 2>/dev/null; then
  sudo iptables -I INPUT -p tcp --dport "$RESOLVER_PORT" -m conntrack --ctstate NEW -j ACCEPT
fi
# Persist across reboots.
echo "iptables-persistent iptables-persistent/autosave_v4 boolean true" | sudo debconf-set-selections
echo "iptables-persistent iptables-persistent/autosave_v6 boolean true" | sudo debconf-set-selections
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y iptables-persistent
sudo netfilter-persistent save

# ---- Done --------------------------------------------------------------------------------
cat <<EOF

============================================================================
 Setup complete. Binaries: $PREFIX/bin  •  Config: /etc/farmore  •  Units installed.

 NEXT (you):
   1) Edit /etc/farmore/node.env and /etc/farmore/resolver.env:
        - paste the deployed Base Sepolia addresses (from farmore-contracts
          deployments/84532.json: settlement, namespace, bondVault, far, collateral)
        - paste FARMORE_OPERATOR_KEY (testnet-only key) into node.env
      Then:  sudo chmod 600 /etc/farmore/node.env   (already 600 if installed by this script)

   2) [HUMAN] Fund the operator address with a little Base Sepolia ETH (gas only).
      Derive it:  /opt/farmore/bin/... uses FARMORE_OPERATOR_KEY; get the address with
        cast wallet address <key>     (collateral + inventory are free via the testnet faucet)

   3) [HUMAN] In the Oracle web console, add an INGRESS rule for TCP $RESOLVER_PORT to the
      instance's Security List / NSG (the instance firewall alone is not enough).

   4) Start both services:
        sudo systemctl enable --now farmore-resolver
        sudo systemctl enable --now farmore-node

   5) Verify:
        curl -s http://localhost:$RESOLVER_PORT/health
        curl -s http://<PUBLIC_IP>:$RESOLVER_PORT/health      # from your laptop
        journalctl -u farmore-node -f
        journalctl -u farmore-resolver -f
============================================================================
EOF
