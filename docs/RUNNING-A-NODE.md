# Running a Farmore Node (Solver / Operator Guide)

**Who this is for:** someone deciding whether to run a Farmore node and become a solver.
**Does it involve financial risk? Yes — real capital, and it can be slashed.** Read the
risk section before anything else.

> ⚠️ **You are putting real money at stake.** A Farmore node bonds collateral and fronts
> its own funds to recipients. If your node asserts a fulfilment that didn't happen (or
> can't be proven), a valid dispute **slashes your bond — you lose that capital.** This is
> operating financial infrastructure, not installing a passive app. Only bond what you can
> afford to lose, and run a reliable, always-online node.

> 🛈 **Mainnet is not live yet.** This describes how it will work. Every live figure below
> is marked **`[SET AT STAGE 2 — measured on testnet, audited before mainnet]`** and is not
> finalized. Do not treat any example as a real value.

## What a solver does, and earns

Running a node is to Farmore what mining is to Bitcoin: you do verifiable work and the
protocol mints **FAR** to you for it. The work is **cross-chain fulfilment** — when a user
says "send 100 USDC to `@amara`", a solver pays Amara instantly on her chain from its own
funds, then gets reimbursed from the source chain. You compete with other solvers to fill
intents; you earn FAR for the ones you fulfil and prove.

FAR is fair-launch: zero genesis supply, no premine, no sale, a fixed **1,000,000,000**
cap, minted only for verified work. Rewards follow three rules (constants pending):

- **Sub-linear in capital** — reward scales with the *square root* of your bond, so more
  capital earns more, but with sharply diminishing returns. Constants:
  `[SET AT STAGE 2 — measured on testnet, audited before mainnet]`.
- **Halving epochs** — the FAR minted per epoch halves on a schedule, so the lifetime sum
  equals the cap. Epoch length: `[SET AT STAGE 2]`.
- **Per-identity epoch cap** — one identity can mint at most a fixed share of an epoch's
  budget, then stops earning until the next epoch. Share: `[SET AT STAGE 2]`.

The point of these rules: no amount of capital can capture issuance. You don't need FAR to
start — you bond ordinary collateral and earn FAR for the work.

## How it works (the loop)

1. **Bond** collateral into the `BondVault` contract on the home chain, and register a
   handle as your identity in `Namespace`.
2. **Watch** the home chain for open intents (ERC-7683 orders).
3. **Front** the recipient on the destination chain, instantly, from your own inventory.
4. **Assert** the fulfilment on the `Settlement` contract, with your bond at stake.
5. A **challenge window** opens. Anyone can dispute. A valid dispute → your bond is
   **slashed** and nothing is minted. Window length: `[SET AT STAGE 2]`.
6. If unchallenged, **finalize** → FAR is minted to you under the rules above, and your
   bond is released.

## What you provide

- **Collateral** to bond — the stake that backs your honesty. Minimum / reference bond:
  `[SET AT STAGE 2 — measured on testnet, audited before mainnet]`.
- **Inventory** — working funds on each destination chain you serve, used to front
  recipients. This is separate from your bond and is recovered as intents settle. How much
  depends on the volume and chains you want to serve: `[SET AT STAGE 2]`.
- **An always-online node** — it must stay running to see intents, front in time, and
  finalize. Recommended machine / RAM / disk / bandwidth:
  `[SET AT STAGE 2 — measured on testnet]`.

## The risks — stated plainly

- **Slashing.** Assert something false or unprovable and a valid dispute takes your bonded
  collateral. The bond is deliberately set higher than what a solver could gain by cheating
  (exact ratio `[SET AT STAGE 2]`), so honest operation is the only profitable strategy.
- **Capital exposure.** Your bond and your inventory are real funds at work. Bugs,
  misconfiguration, downtime during a fill, or a destination-chain failure can cost money.
- **You are the operator.** No one backstops your node. Keys, uptime, and funding are your
  responsibility.

## Honest constraints

- The network can only move what solvers can **collectively front** on the destination
  chain. If you serve a chain, you can only fill what your inventory there covers.
- **Large transfers to thin chains** — especially Bitcoin — may be slow or unfillable until
  solver liquidity grows. This is a real limit, not a bug.
- A bond must always be worth more than what cheating could steal; the protocol enforces
  this, and you should size your own positions with the same logic.

## Running the node today

The node is a Rust daemon. Today it runs from source / a release binary; a friendlier path
is coming (see the next section).

1. **Install** the toolchain: [Foundry](https://book.getfoundry.sh) (`forge`, `anvil`,
   `cast`) and Rust (pinned in `rust-toolchain.toml`). See the repo
   [README](../README.md).
2. **Build:** `cargo build --release` (the `farmore-node` binary).
3. **Configure** entirely via environment — never hard-code keys. Copy
   [`.env.example`](../.env.example) to `.env` and fill in your RPC, operator key, the
   deployed contract addresses, your handle, and your target bond + inventory. Mainnet
   addresses / chain IDs / RPC: `[SET AT STAGE 3 — published at mainnet bootstrap]`.
4. **Fund** your operator account with gas on each chain you serve, plus your collateral
   and inventory. (On a testnet, set `FARMORE_FAUCET=true` to self-fund from the test
   faucet.) Amounts: `[SET AT STAGE 2]`.
5. **Bond & run:** `cargo run --release --bin farmore-node`. On start it registers your
   handle and tops up your bond, then runs the loop. It is crash-safe and idempotent —
   restarting never double-spends or double-asserts.
6. **Monitor.** The node emits structured logs (bonded / fronted / asserted / finalized).
   Watch that it stays online and that your bond and inventory are healthy. A status
   dashboard is planned (below).

## Product direction: easy to run, honest about stakes

A core goal of Farmore is that **average people, without advanced technical skill, can run
a node and be a solver.** The following are the intended direction — **planned, not yet
shipped:**

- A **one-click desktop node app** that generates and secures your keys, walks you through
  funding and bonding with plain-language risk explanations, and runs in the background
  with a simple status dashboard (**bonded / earning / online**).
- Guided setup that picks sane defaults and explains every choice.

The principle we hold ourselves to: **make it easy to operate, but impossible to miss the
risk.** Lowering the technical bar must never hide that a solver can be slashed and lose
capital. Easy to run; honest about the stakes.

## Status

Mainnet is not live. Stage 2 runs the full loop on a public testnet, where the bond,
inventory, hardware, and timing values above are measured and then audited before any
mainnet launch. Until then, treat every `[SET AT STAGE 2/3]` marker as genuinely unknown.
