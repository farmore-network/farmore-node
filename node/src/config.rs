//! Node configuration, sourced entirely from the environment (never hard-coded), with
//! a programmatic constructor for tests. Secrets (the operator key) are read from env
//! and never logged.

use alloy::primitives::{Address, U256};
use anyhow::{Context, Result};

/// Runtime configuration for a Farmore node.
#[derive(Clone)]
pub struct Config {
    pub home_rpc_url: String,
    pub operator_key: String,
    pub settlement: Address,
    pub namespace: Address,
    pub bond_vault: Address,
    pub collateral: Address,
    pub far: Address,
    /// The operator's handle (its identity for the per-identity cap).
    pub handle: String,
    /// Target collateral bond to maintain, in the collateral token's base units.
    pub bond_amount: U256,
    /// Working inventory to hold in the operator wallet for fronting recipients
    /// (separate from the bond). On a testnet with `faucet=true`, the node tops this up
    /// from the TestUSD faucet; on mainnet the operator funds its own inventory.
    pub front_inventory: U256,
    /// Destination chain id this node serves (defaults to the home chain id in M1).
    pub dest_chain_id: Option<u64>,
    /// Collateral decimals (USDC-like = 6).
    pub collateral_decimals: u8,
    /// Confirmations required to treat a destination fill as final.
    pub finality_confirmations: u64,
    /// Poll interval between ticks, milliseconds.
    pub poll_ms: u64,
    /// Block to begin scanning from (defaults to current head at startup).
    pub start_block: Option<u64>,
    /// Path to the crash-safe state file.
    pub state_file: String,
    /// On a testnet, mint collateral from the TestUSD faucet to top up the bond.
    pub faucet: bool,
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing env var {key}"))
}

fn env_addr(key: &str) -> Result<Address> {
    env(key)?
        .parse()
        .with_context(|| format!("invalid address in {key}"))
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

impl Config {
    /// Loads configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            home_rpc_url: env("FARMORE_HOME_RPC_URL")?,
            operator_key: env("FARMORE_OPERATOR_KEY")?,
            settlement: env_addr("FARMORE_SETTLEMENT")?,
            namespace: env_addr("FARMORE_NAMESPACE")?,
            bond_vault: env_addr("FARMORE_BOND_VAULT")?,
            collateral: env_addr("FARMORE_COLLATERAL")?,
            far: env_addr("FARMORE_FAR")?,
            handle: env("FARMORE_HANDLE")?,
            bond_amount: env("FARMORE_BOND_AMOUNT")?
                .parse()
                .context("invalid FARMORE_BOND_AMOUNT")?,
            front_inventory: env_opt("FARMORE_FRONT_INVENTORY")
                .map(|v| v.parse())
                .transpose()
                .context("invalid FARMORE_FRONT_INVENTORY")?
                .unwrap_or(U256::ZERO),
            dest_chain_id: env_opt("FARMORE_DEST_CHAIN_ID")
                .map(|v| v.parse())
                .transpose()?,
            collateral_decimals: env_opt("FARMORE_COLLATERAL_DECIMALS")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(6),
            finality_confirmations: env_opt("FARMORE_FINALITY_CONFIRMATIONS")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(1),
            poll_ms: env_opt("FARMORE_POLL_MS")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(2000),
            start_block: env_opt("FARMORE_START_BLOCK")
                .map(|v| v.parse())
                .transpose()?,
            state_file: env_opt("FARMORE_STATE_FILE")
                .unwrap_or_else(|| "farmore-node-state.json".into()),
            faucet: env_opt("FARMORE_FAUCET")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
        })
    }
}
