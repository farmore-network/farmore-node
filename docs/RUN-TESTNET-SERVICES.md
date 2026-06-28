# Farmore — Run the Testnet Services 24/7 on Oracle Cloud (Ampere A1 / ARM64)

Stand up the **resolver** and the **node** so they run 24/7 on an Oracle Cloud Always Free
**Ampere A1 (ARM64 / aarch64) Ubuntu** server, pointed at the Base Sepolia contracts you
deployed in `farmore-contracts/docs/TESTNET-DEPLOY-RUNBOOK.md`.

> Free, fake money. Testnet only. This is light but **real ops**: you keep two services up,
> watch their logs, and the node holds a (testnet) operator key. See the
> [Operations](#operations--logs) section.

Steps are **[HUMAN]** when they need your Oracle account, the browser console, or your own
funds/keys. Everything else is one `setup-oracle.sh` run plus two `systemctl` commands.

## The two services

| Service | What it does | Network | Holds funds/keys? |
|---------|--------------|---------|-------------------|
| **farmore-resolver** | Read-only HTTP API: handle resolution, send-to-handle order prep, sign-in. Exposes `GET /health`. | **Listens** on TCP **8080** (inbound) + outbound RPC | No |
| **farmore-node** | Bonds collateral, watches Settlement for intents, fronts recipients, asserts, finalizes, earns FAR. Crash-safe daemon. | **Outbound only** (RPC); listens on **no** port | Yes — operator key |

Because only the resolver listens, **only one port (8080) needs opening**. The node opens no port.

## Why no prebuilt binary / no cross-compilation

The Ampere A1 is itself aarch64, so `setup-oracle.sh` builds the binaries **natively on the box**.
That makes them aarch64 ELF by construction — no cross-compiler, no `cross`, and no
"exec format error" from an arch mismatch. (CI does not publish release artifacts, so building on
the box is also simply the supported path.)

The node/resolver Cargo workspace has path dependencies on two sibling repos
(`../farmore-core`, `../farmore-ethereum-adapter`), so the setup script checks out all three repos
side by side before building. `farmore-contracts` is **not** needed to build the binaries (only the
e2e tests reference it, at runtime).

---

## Part A — [HUMAN] Create the Oracle instance (web console)

1. **Create the instance.** Console → **Compute → Instances → Create instance**.
   - **Image & shape:** choose **Ubuntu** (22.04 or 24.04) and shape **Ampere — VM.Standard.A1.Flex** (this is the ARM64 Always Free shape). 1 OCPU / 6 GB is enough; up to 4 OCPU / 24 GB is within the free allowance and builds faster.
   - **"Out of host capacity" error?** This is a well-known Oracle quirk for free A1, **not** a mistake on your part. Retry, switch the **Availability Domain** (AD-1/2/3) in the dialog, or try a different home region. Retrying over time usually succeeds.
2. **SSH key.** In the **Add SSH keys** panel, either let Oracle **generate a key pair and download the private key**, or paste your own public key. Save the private key (e.g. `~/.ssh/oracle_farmore`).
3. **Record the public IP.** After it boots, copy the instance's **Public IP address** from the instance details page.
4. **Open the resolver port in the cloud firewall (Security List or NSG).** This is the cloud-level layer — separate from the instance's own firewall.
   - Instance details → **Virtual Cloud Network** → **Security Lists** → the subnet's **Default Security List** → **Add Ingress Rules**. (Or use an NSG attached to the VNIC.)
   - Add an ingress rule with **exactly**:
     - **Stateless:** No
     - **Source Type:** CIDR
     - **Source CIDR:** `0.0.0.0/0`  *(or lock to your IP/`32` for a private test)*
     - **IP Protocol:** TCP
     - **Source Port Range:** (leave blank / All)
     - **Destination Port Range:** `8080`
   - SSH (TCP 22) is allowed by default; leave it.

> **Two firewall layers.** Oracle blocks the port at **both** the cloud Security List/NSG (this
> step) **and** the instance's own iptables. `setup-oracle.sh` handles the instance layer. If
> `/health` is unreachable from your laptop but works via `curl localhost:8080` on the box, it is
> almost always this cloud-level ingress rule (or a port mismatch).

5. **SSH in:**
   ```bash
   chmod 600 ~/.ssh/oracle_farmore
   ssh -i ~/.ssh/oracle_farmore ubuntu@<PUBLIC_IP>
   ```

---

## Part B — Run the setup script (on the server, over SSH)

Fetch this repo and run the setup script. It installs deps, (creates swap if RAM is low), installs
Rust, clones the three sibling repos, **builds the aarch64 binaries**, installs them + the systemd
units, writes config templates, and opens TCP 8080 in the **instance** firewall.

```bash
# On the Oracle box, as the default 'ubuntu' user (the script calls sudo itself):
git clone --depth 1 https://github.com/farmore-network/farmore-node.git
bash farmore-node/deploy/setup-oracle.sh
```

- Override defaults via env if needed, e.g. a different port or private clone over SSH:
  ```bash
  RESOLVER_PORT=8080 GIT_BASE=git@github.com:farmore-network bash farmore-node/deploy/setup-oracle.sh
  ```
- **Success looks like:** the build finishes, `file target/release/farmore-*` reports
  `ELF 64-bit LSB ... ARM aarch64`, and the script prints the `Setup complete` banner with the
  NEXT steps.
- **Most likely failures + fixes:**
  - **Build killed / OOM on a 1-OCPU box** → the script creates a 4 GB swapfile automatically; if you skipped it, add swap and re-run. Re-running is safe (idempotent).
  - **`Permission denied (publickey)` cloning** → the repos are under `github.com/farmore-network`; if private, use `GIT_BASE=git@github.com:farmore-network` with an SSH key/agent, or a token.

The script is **idempotent** — to ship a code update later: re-run it (it pulls `main`, rebuilds,
reinstalls), then `sudo systemctl restart farmore-node farmore-resolver`.

---

## Part C — Configure the services

The setup script installed two config files from the templates in `deploy/`. Fill them in with the
addresses from your deploy (`farmore-contracts/deployments/84532.json` → `addresses/84532.json`).

```bash
sudo nano /etc/farmore/resolver.env   # FARMORE_NAMESPACE, FARMORE_SETTLEMENT, FARMORE_COLLATERAL
sudo nano /etc/farmore/node.env       # the 5 addresses + FARMORE_OPERATOR_KEY
```

- The five addresses the node needs: `FARMORE_SETTLEMENT`, `FARMORE_NAMESPACE`,
  `FARMORE_BOND_VAULT`, `FARMORE_FAR`, `FARMORE_COLLATERAL`.
- `node.env` holds the **operator private key** — it is installed `chmod 600`, owned by root
  (systemd reads it as root before dropping to the `farmore` user). Keep it 600.
- Comments must be on their own lines (systemd does **not** strip inline `# ...` after a value).

### [HUMAN] Node operator funding + bonding

The node **bonds itself automatically** on first start (`bootstrap()`): it registers its handle,
and on testnet (`FARMORE_FAUCET=true`) **self-funds collateral and inventory from the TestUSD
faucet**, then deposits the bond into BondVault. So:

- **You only need to fund the operator address with a little Base Sepolia ETH for gas.** Get the
  address from the key you put in `node.env`:
  ```bash
  cast wallet address <FARMORE_OPERATOR_KEY>
  ```
  Send it ~0.05 test ETH from a faucet (same faucets as the contracts runbook).
- **Bond amount:** testnet — **operator's choice (free)**. The template default is
  `FARMORE_BOND_AMOUNT=1000000000` (1,000 tUSD, the reference bond). Raise/lower freely; on
  testnet the collateral is faucet-minted. `[SET AT STAGE 2]` for any real mainnet figure.
- No separate "bond" or "watch" command exists — bonding and watching are automatic on start.

---

## Part D — Start the services

```bash
sudo systemctl enable --now farmore-resolver
sudo systemctl enable --now farmore-node
```

`enable --now` starts them immediately **and** sets them to start on boot; both units restart on
failure.

---

## Go-live verification

```bash
# Resolver reachable locally on the box:
curl -s http://localhost:8080/health
# Expect: {"status":"ok","chainId":84532,"namespace":"0x...","settlement":"0x..."}

# Resolver reachable from the public internet (run from your laptop):
curl -s http://<PUBLIC_IP>:8080/health
```

- [ ] **Resolver up & reachable** from the internet (`/health` returns `chainId: 84532` and your addresses).
- [ ] **Node up & bonded** — `journalctl -u farmore-node` shows `registered identity` (first run) / `topped up bond` and then `node running`.
- [ ] **Node watching for intents** — `node running` with periodic ticks; no repeating errors.
- [ ] **One real end-to-end transfer between two test handles** completes — see below.

### The end-to-end transfer (recipient receives funds; record the tx hash)

With the node running and watching, exercise the live send-to-handle path:

1. Register a **recipient** handle (any wallet) in the Namespace, and map its receive address.
2. Ask the resolver to prepare the order to that handle:
   ```bash
   curl -s -X POST http://<PUBLIC_IP>:8080/send \
     -H 'content-type: application/json' \
     -d '{"to_handle":"<recipient_handle>","asset":"USDC","amount":"100000000"}'
   ```
3. Submit the returned ERC-7683 order via `openIntent` from a **sender** wallet (cast/SDK).
4. The running node detects the intent, **fronts the recipient** on Base Sepolia (the recipient
   receives the funds), asserts, waits the 60s window, and finalizes — earning FAR. Watch:
   ```bash
   journalctl -u farmore-node -f          # look for asserted/finalized counts
   ```
5. **Record the fill transaction hash** (from the node logs / the recipient's incoming transfer on
   Basescan) as the proof of a completed transfer.

> A quicker protocol-loop proof (recipient = a fixed test address, not a handle) is
> `make smoke-sepolia` in `farmore-contracts`. The handle→handle flow above is the user-facing
> end-to-end and the one to record for go-live.

---

## Operations — logs

This is light but real ops. Keep the services up and glance at logs.

```bash
# Live logs
journalctl -u farmore-node -f
journalctl -u farmore-resolver -f

# Recent errors only
journalctl -u farmore-node -p err --since "1 hour ago"

# Service status / restarts
systemctl status farmore-node farmore-resolver

# Restart after a config edit or code update
sudo systemctl restart farmore-node farmore-resolver
```

- **Node state journal:** `/var/lib/farmore/farmore-node-state.json` — crash-safe; the node resumes
  exactly where it left off (no double-fronting) after a restart or reboot.
- **Resolver health:** poll `GET /health`. Put a TLS-terminating reverse proxy (Caddy/nginx) in
  front of it if you want HTTPS and a hostname; not required for the testnet check.
- **If `/health` is unreachable from outside but fine via `curl localhost:8080`:** it's one of the
  two firewall layers — re-check the **Oracle Security List/NSG ingress** (Part A step 4) and that
  `RESOLVER_BIND` / the opened port / the Oracle rule all use the **same** port.

---

## Publish (close the loop)

Once both services are verified, publish for others:

- The deployed addresses (`addresses/84532.json` from the contracts repo).
- A short "how to join / run a testnet node" note — point operators at this file and
  `docs/RUNNING-A-NODE.md` / `docs/RUNNING-A-RESOLVER.md`.

## Quick reference

| Thing | Value |
|-------|-------|
| Binaries | `/opt/farmore/bin/farmore-node`, `/opt/farmore/bin/farmore-resolver` |
| Config | `/etc/farmore/node.env` (0600), `/etc/farmore/resolver.env` |
| Units | `/etc/systemd/system/farmore-{node,resolver}.service` |
| State | `/var/lib/farmore/farmore-node-state.json` |
| Resolver port | TCP **8080** — open in Oracle NSG **and** instance iptables |
| Source checkout | `~/farmore/src/{farmore-node,farmore-core,farmore-ethereum-adapter}` |
