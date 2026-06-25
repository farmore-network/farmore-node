# Running a Farmore Resolver (Operator Guide)

**Who this is for:** anyone who wants to run a Farmore resolver — a public read/index
service.
**Does it involve financial risk? No.** The resolver holds no funds, bonds nothing, and
cannot be slashed. It is safe to run as a public good.

> 🛈 **Mainnet is not live yet.** Mainnet addresses / chain IDs / RPC endpoints are
> **`[SET AT STAGE 3 — published at mainnet bootstrap]`**.

## What it is

The resolver is a small HTTP service that reads on-chain state and serves:

- **Handle resolution** — look up a `@handle` and its cross-chain account.
- **Send-to-handle** — prepare an ERC-7683 order to pay a handle (it never holds or moves
  funds; the caller signs and submits).
- **Sign-in with handle** — a challenge/verify flow proving handle ownership.

It is stateless apart from short-lived sign-in nonces. **The source of truth always stays
on-chain** — the resolver only reads and indexes it, so running one is non-custodial and
anyone can run one. More resolvers means a more resilient, censorship-resistant network.

## Run it

The resolver ships as a second binary in this repo and runs **on its own — no node, no
bond, no collateral required.**

```bash
cargo run --release --bin farmore-resolver
```

Configure via environment (see [`.env.example`](../.env.example)) — at minimum the home
RPC URL and the `Namespace`, `Settlement`, and collateral addresses, plus `RESOLVER_BIND`
(listen address). It exposes:

```
GET  /health
GET  /resolve/:handle
GET  /resolve/:handle/:asset
POST /send
GET  /signin/:handle/nonce
POST /signin/verify
```

Run it behind your own TLS/proxy and monitor `/health`. Because it is read-only, the worst
case from downtime is unavailability of *your* endpoint — never loss of funds.

## Status

Mainnet is not live. On testnet the resolver runs against the testnet deployment; mainnet
endpoints are published at bootstrap.
