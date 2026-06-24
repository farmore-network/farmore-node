# Farmore Node — workspace tasks.
export PATH := $(HOME)/.foundry/bin:$(PATH)

# Pinned farmore-contracts location for the cross-stack e2e (sibling checkout by default).
FARMORE_CONTRACTS_DIR ?= $(abspath ../farmore-contracts)
export FARMORE_CONTRACTS_DIR

.PHONY: help build test e2e fmt lint node resolver
help:
	@echo "make build    - cargo build --workspace"
	@echo "make test     - cargo test --workspace (unit + cross-stack e2e)"
	@echo "make e2e       - cross-stack e2e: anvil + real farmore-contracts + node loop + slash path"
	@echo "make fmt lint  - format / clippy + fmt check"
	@echo "make node      - run the node daemon (config from env; see .env.example)"
	@echo "make resolver  - run the resolver service standalone"
	@echo "  (FARMORE_CONTRACTS_DIR=$(FARMORE_CONTRACTS_DIR))"

build:
	cargo build --workspace

test: e2e

# Spins anvil, deploys the real contracts from FARMORE_CONTRACTS_DIR, runs the node through
# bond -> intent -> front -> assert -> finalize -> mint, plus the slash path. Requires
# forge + anvil on PATH and farmore-contracts checked out (sibling or FARMORE_CONTRACTS_DIR).
e2e:
	cargo test --workspace

fmt:
	cargo fmt --all

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

node:
	cargo run --bin farmore-node

resolver:
	cargo run --bin farmore-resolver
