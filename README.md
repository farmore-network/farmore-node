# farmore-node

The **Farmore Node** and **resolver** — a Rust workspace with two binaries:

- **`farmore-node`** — the daemon operators run to earn FAR. It bonds collateral, watches
  the home chain for open intents, fronts recipients on the destination chain from its own
  funds (via the chain-neutral [`farmore-core`](https://github.com/farmore-network/farmore-core)
  adapter), asserts fulfilment, and finalizes after the challenge window to mint FAR.
  Running it is to Farmore what mining is to Bitcoin — **permissionless: anyone can run one.**
- **`farmore-resolver`** — the resolver/indexer HTTP service: handle resolution,
  send-to-handle order preparation, and sign-in by handle ownership. **Runs standalone**
  (no node required): `cargo run --bin farmore-resolver`.

Production-daemon properties: no `unwrap`/`expect`/`panic!` in any running path; crash-safe
and idempotent (journaled steps + on-chain status guards); RPC retries with backoff;
graceful shutdown; structured `tracing` logs.

## Run a node

```bash
cp .env.example .env     # fill in RPC, operator key, deployed addresses, bond/inventory
set -a; source .env; set +a
cargo run --bin farmore-node
```

Key settings (env, never hard-coded — see `.env.example`): `FARMORE_HOME_RPC_URL`,
`FARMORE_OPERATOR_KEY`, the contract addresses, `FARMORE_HANDLE`, `FARMORE_BOND_AMOUNT`,
`FARMORE_FRONT_INVENTORY`. On a testnet set `FARMORE_FAUCET=true` to self-fund bond +
inventory from the TestUSD faucet. **Never commit your key** — the operator key is read
from the environment / a secrets manager only.

## Run the resolver standalone

```bash
FARMORE_HOME_RPC_URL=... FARMORE_NAMESPACE=0x... FARMORE_SETTLEMENT=0x... \
FARMORE_COLLATERAL=0x... RESOLVER_BIND=0.0.0.0:8080 \
  cargo run --bin farmore-resolver
# GET /health  GET /resolve/:handle  POST /send  GET /signin/:handle/nonce  POST /signin/verify
```

## Build, test, and the cross-stack e2e

```bash
make build
make e2e     # anvil + real contracts + full loop (mint) + slash path
make lint    # cargo fmt --check && cargo clippy -- -D warnings
```

The e2e test deploys the **real** `farmore-contracts` and runs the node end to end. It
needs `forge` + `anvil` on PATH and the contracts available. Provide them by checking out
`farmore-contracts` as a sibling directory (default) or setting `FARMORE_CONTRACTS_DIR`:

```
farmore/
  farmore-core/
  farmore-ethereum-adapter/
  farmore-contracts/        # pinned; `forge soldeer install` once to fetch its deps
  farmore-node/             <- you are here
```

## Cross-repo dependency model

Depends on `farmore-core` and `farmore-ethereum-adapter` via **path + version** deps for
the sibling-checkout dev layout above; switch to the published/git-tag versions once they
are released (see those repos' READMEs). `Cargo.lock` is committed for reproducible builds.

## License

MIT — see [LICENSE](LICENSE).
