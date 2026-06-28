# Farmore — Base Sepolia Testnet: Live Status, How It Works, Onboarding

_Last updated: 2026-06-28. Network: **Base Sepolia (chain id 84532)**. Money: **free, fake** (test ETH + faucet TestUSD). No audit, no multisig, no real funds._

---

## 1. TL;DR — what's live

- **Contracts:** deployed and **verified** on Basescan (FARToken, Namespace, BondVault, Settlement, TestUSD).
- **Resolver:** running 24/7 on Oracle Cloud (ARM64), reachable at `http://84.8.134.52:8080`.
- **Node (solver):** running 24/7, bonded, watching for intents, fronting + finalizing autonomously.
- **Proven end-to-end:** two live handle→handle transfers completed — recipients received funds within seconds, the node finalized after the challenge window, and **200 FAR** has been minted as solver rewards.

What's **not** there yet: a user-facing wallet UI, TLS/domain on the resolver, multiple independent nodes, cross-chain, and the resilience fixes still need to be merged + redeployed (see §9).

---

## 2. Deployed contracts (Base Sepolia, 84532)

| Contract | Address | Explorer |
|----------|---------|----------|
| **Settlement** | `0x3f6E6440d5bD273FCE9e2A235837851e8292B482` | https://sepolia.basescan.org/address/0x3f6E6440d5bD273FCE9e2A235837851e8292B482#code |
| **FARToken (FAR)** | `0x4cAA55C51814E3989bc9B2A05C47AA2D075Ba9A0` | https://sepolia.basescan.org/token/0x4cAA55C51814E3989bc9B2A05C47AA2D075Ba9A0 |
| **Namespace** | `0xCdbE2bce9d4e57B2834d8D1B4Fe2A2F000Ef3654` | https://sepolia.basescan.org/address/0xCdbE2bce9d4e57B2834d8D1B4Fe2A2F000Ef3654#code |
| **BondVault** | `0x870FBCFba4F6C51F471fe31A0cD0a623e3e8f3BA` | https://sepolia.basescan.org/address/0x870FBCFba4F6C51F471fe31A0cD0a623e3e8f3BA#code |
| **TestUSD (collateral, tUSD)** | `0x1e14bC449Af174e3C70a1e8C90b69b1E867CC305` | https://sepolia.basescan.org/address/0x1e14bC449Af174e3C70a1e8C90b69b1E867CC305#code |

- Canonical machine-readable copy: `farmore-contracts/addresses/84532.json`.
- `arbiter` and `treasury` = the deployer (testnet default).

---

## 3. Services running 24/7

| Service | Where | Endpoint / identity | Holds |
|---------|-------|---------------------|-------|
| **farmore-resolver** | Oracle A1 (ARM64, Ubuntu 24.04), systemd | `http://84.8.134.52:8080` (`/health` OK) | nothing (read-only) |
| **farmore-node** | same box, systemd | operator `0xeBF92F37effA918E07151fDa6B49Ce27367f65C5`, handle `node1` | operator key + bond |

- Both `systemctl enable`d → start on boot, restart on failure.
- Logs: `journalctl -u farmore-resolver -f` / `journalctl -u farmore-node -f`.
- Node state journal (crash-safe): `/var/lib/farmore/farmore-node-state.json`.
- Node bond: maintained at 1,000 tUSD target (currently 2,000 from an earlier top-up artifact — harmless). Working inventory ~10,000 tUSD (faucet-funded).

---

## 4. What's been tested (with evidence)

- **Build/format/tests (local):** `forge build` clean; `forge fmt` clean; **97 contract tests pass** incl. the bond-capacity invariant and supply-accounting invariants. Node workspace: `cargo clippy -D warnings` clean.
- **Deploy + verify (live):** all 5 contracts deployed and source-verified on Basescan; on-chain wiring checked (`FAR.minter == Settlement`, `BondVault.settlement == Settlement`, challenge window = 60s).
- **Resolver (live):** `/health` and `/resolve/:handle` answer from the public internet.
- **Node bootstrap (live):** registered its handle, bonded collateral, funded inventory — all verified on-chain.
- **End-to-end money path (live, twice):** intent opened → node fronted the recipient (funds arrived in ~4s) → asserted → 60s challenge window → finalized → **FAR minted**. Total supply now **200 FAR**, held by the solver. (See the table in this PR/commit message for tx hashes.)

---

## 5. How the system works

Farmore lets someone **send money to a human-readable handle** (like `@alice`) instead of a hex address, and have it delivered **instantly** by a solver who is later rewarded in FAR. It's an optimistic, intent-based design (ERC-7683 orders).

### Roles
- **Recipient** — registers a handle and a receive address; just receives funds.
- **Sender** — opens an intent: "pay X of asset Y to handle Z". Pays only gas.
- **Solver / node operator** — bonds collateral, watches for intents, **fronts** recipients from its own inventory, and earns **FAR** for honest service.
- **Resolver** — a read-only helper service that maps handles→addresses and prepares orders for wallets. Holds no funds, runs anywhere.

### The flow (what happened in our live tests)
1. **Recipient sets up** — `Namespace.register("alice")`, then `setDefaultReceiver("alice", 84532, <address>)`. Now `alice` resolves to an address on Base Sepolia.
2. **Sender prepares** — the wallet calls the resolver `POST /send {to_handle, asset, amount}`; the resolver resolves `alice`→address and returns the ERC-7683 order fields.
3. **Sender opens the intent** — `Settlement.openIntent(order)`. This just publishes the request on-chain (no escrow, no bond from the sender).
4. **Node fronts** — the running node sees the open intent within a few seconds, **transfers the funds to the recipient from its own inventory** (recipient is paid immediately), and calls `assertFulfillment`, staking its bond.
5. **Challenge window** — 60 seconds (testnet) in which anyone could dispute the assertion (optimistic security). Undisputed assertions become finalizable.
6. **Finalize → reward** — the node calls `finalize`; Settlement mints **FAR** to the solver under the fairness rules. The recipient already had the money; FAR is the solver's incentive.

### Safety mechanism — the bond capacity invariant
A solver's total outstanding fronting exposure must stay within `capacityFraction × live bond` (50% on testnet), with a bond floor to take work, enforced in BondVault. Exposure is released exactly once on finalize or slash. This is what keeps solvers honest and over-collateralized; it's covered by an invariant fuzz test.

### The FAR token
- Symbol **FAR**, 18 decimals, **1,000,000,000 cap**, **zero genesis supply**.
- Minted **only** by Settlement, **only** on finalize, under sub-linear-in-bond + halving-epoch + per-identity-cap rules.
- That's why supply is small and grows one finalize at a time (100 FAR each at the reference bond right now).

---

## 6. Economic parameters (testnet config)

| Parameter | Testnet value | Notes |
|-----------|---------------|-------|
| Challenge window | **60 s** | mainnet would be hours |
| Base reward | **100 FAR** at reference bond | reference bond = 1,000 tUSD |
| FAR multiplier (FAR-bonded) | 1.5× | not used yet (collateral bonds only) |
| Per-identity epoch cap | 1% of epoch budget | |
| Epoch (halving) | 730 days | |
| Capacity fraction | 50% over-collateralization | enforced in BondVault |
| Dispute bond | 10% of asserter bond | |
| Slash → challenger | 50% of slashed bond | |
| Collateral | TestUSD (6 dp), public faucet | mainnet would use real USDC |

Parameters are immutable in this deployment.

---

## 7. How real users will use it

The protocol is live, but non-technical users need a UI. Two paths:

### A) Via the Farmore wallet (the build to finish for real testers)
The wallet should wrap the same calls our test used:
- **Onboard / claim a handle:** `Namespace.register(handle)` + `setDefaultReceiver(handle, chainId, address)`.
- **Sign in with handle:** resolver `GET /signin/:handle/nonce` → sign → `POST /signin/verify` (proves handle ownership, no funds moved).
- **Send to a handle:** resolver `POST /send {to_handle, asset, amount}` → wallet submits `Settlement.openIntent(order)`.
- **Receive:** nothing to do — the node fronts the funds to the user's receive address within seconds.
- **Read:** resolver `GET /resolve/:handle` and `/resolve/:handle/:asset`.

Integration points for the wallet: the **resolver base URL** (`http://84.8.134.52:8080`) and the **contract addresses** in §2 (also in `farmore-contracts/addresses/84532.json` and the published ABI package).

### B) Manual path (works today, for technical testers)
Using `cast` (Foundry) against Base Sepolia, a tester can: get test ETH from a faucet, `register` a handle, `setDefaultReceiver`, then either receive (have someone open an intent to their handle) or send (`openIntent`). This is exactly what our end-to-end test script does.

---

## 8. How to run a node (operator onboarding)

Anyone can run a solver. The full guide is `farmore-node/docs/RUN-TESTNET-SERVICES.md` (Oracle ARM64) and `docs/RUNNING-A-NODE.md`. In short:
1. Build the `farmore-node` binary (native on the target box; ARM64 supported).
2. Fund an operator address with a little Base Sepolia ETH (gas). On testnet the node self-funds collateral + inventory from the TestUSD faucet.
3. Configure `/etc/farmore/node.env` with the §2 addresses + the operator key, then `systemctl enable --now farmore-node`.
4. The node auto-registers its handle, bonds, and starts earning FAR by serving intents.

More independent nodes = more reliable fronting and faster delivery (today there is a single node, so if it's down, intents aren't served until it's back).

---

## 9. Outstanding before wider testing

1. **Merge + redeploy the resilience fixes** (coded + locally verified, not yet on `main`):
   - `farmore-node`: RPC timeouts (no more hangs), self-healing finalize (re-read `finalizableAt`), total-bond top-up (`bondOf`), resolver `/send` field aliases, systemd `StartLimit` fix. → PR `fix/node-rpc-resilience`, then re-run `setup-oracle.sh` + restart.
   - `farmore-contracts`: Etherscan v2 `chainid` in `foundry.toml`, `verify-sepolia` flags, published `addresses/84532.json`. → PR `fix/verify-and-addresses`.
   > Until merged + redeployed, the live node is the pre-fix build; it works when the public RPC behaves, but the hardening prevents the hang/stale-read we hit.
2. **Dedicated RPC** (optional after the fix): set `FARMORE_HOME_RPC_URL` to an Alchemy/QuickNode Base Sepolia endpoint for fewer retries.
3. **TLS + domain on the resolver** for a public test (put Caddy/nginx in front of `:8080`).
4. **Farmore wallet** UI for non-technical testers (§7A).
5. **More nodes** for reliability.

---

## 10. Caveats & security (read before inviting testers)

- **Testnet only — fake money.** Tell testers never to send real funds; tUSD is a faucet token and FAR has no value.
- **Single operator/node** — if the box or node is down, sends aren't fronted until it recovers.
- **Single chain** — home == destination == Base Sepolia; cross-chain isn't exercised yet.
- **Resolver is plain HTTP** (no TLS) and IP-only — fine for a closed test, not for a public launch.
- **Operator key lives on the server** (testnet key). For mainnet: secrets manager/HSM, never a raw key on disk.
- **Handles are first-come** — expect squatting in an open test.
- **No audit, immutable params** — this is a dress rehearsal, not production.

---

## 11. Quick reference

| Thing | Value |
|-------|-------|
| Network | Base Sepolia, chain id **84532** |
| Resolver | `http://84.8.134.52:8080` (`/health`, `/resolve/:handle`, `/resolve/:handle/:asset`, `/send`, `/signin/...`) |
| Settlement | `0x3f6E6440d5bD273FCE9e2A235837851e8292B482` |
| FAR token | `0x4cAA55C51814E3989bc9B2A05C47AA2D075Ba9A0` (supply 200 FAR / cap 1B) |
| Namespace | `0xCdbE2bce9d4e57B2834d8D1B4Fe2A2F000Ef3654` |
| BondVault | `0x870FBCFba4F6C51F471fe31A0cD0a623e3e8f3BA` |
| TestUSD (collateral) | `0x1e14bC449Af174e3C70a1e8C90b69b1E867CC305` (public `faucet(address,uint256)`) |
| Node operator | `0xeBF92F37effA918E07151fDa6B49Ce27367f65C5`, handle `node1` |
| Server | Oracle A1 ARM64, Ubuntu 24.04, region af-johannesburg-1 |
| Faucets | Base Sepolia ETH: CDP / Chainlink / QuickNode; tUSD: contract `faucet()` |
