//! # farmore-node
//!
//! The Farmore Node — the daemon that earns FAR. It bonds collateral, watches the home
//! chain for open intents, fronts recipients on the destination chain from its own
//! funds through the chain-neutral [`farmore_core::ChainAdapter`], asserts fulfilment,
//! and finalizes after the challenge window to mint FAR.
//!
//! Production-daemon properties:
//! - **No panics in the running path.** Per-order failures are logged and isolated; the
//!   loop never unwinds on a single bad order or a transient RPC error.
//! - **Crash-safe & idempotent.** Every step is journaled (see [`state`]); a restart
//!   resumes without re-fronting or double-asserting. Assertions are guarded by the
//!   on-chain order status, so they are idempotent against the contract too.
//! - **Resilient RPC.** Reads and sends retry with exponential backoff.
//! - **Graceful shutdown** on SIGINT.

#![forbid(unsafe_code)]

pub mod config;
pub mod state;

use std::time::Duration;

use alloy::eips::BlockId;
use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::providers::{DynProvider, Provider};
use anyhow::{anyhow, bail, Context, Result};
use farmore_core::{
    AssetInfo, AssetRegistry, Capabilities, Capable, FinalityPolicy, Settler, TransferRequest,
};
use farmore_ethereum_adapter::{
    build_provider, EvmAdapter, IBondVault, IERC20Faucet, INamespace, ISettlement,
};
use tracing::{error, info, warn};

pub use config::Config;
use state::{NodeState, OrderRecord, Stage};

/// Home-chain settlement status codes (mirror of `ISettlement.Status`).
mod status {
    pub const OPENED: u8 = 1;
    pub const ASSERTED: u8 = 2;
    pub const FINALIZED: u8 = 4;
    pub const SLASHED: u8 = 5;
}

/// Retries an async RPC operation with bounded exponential backoff.
async fn retry<T, F, Fut>(label: &str, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay = Duration::from_millis(250);
    for attempt in 1..=5u32 {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) if attempt == 5 => return Err(e.context(format!("{label}: retries exhausted"))),
            Err(e) => {
                warn!(target: "farmore::node", label, attempt, error = %e, "rpc retry");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(8));
            }
        }
    }
    unreachable!()
}

/// The Farmore Node.
pub struct Node {
    cfg: Config,
    provider: DynProvider,
    account: Address,
    adapter: EvmAdapter,
    dest_chain: u64,
    handle_node: B256,
    registry: AssetRegistry,
    challenge_window: u64,
    state: NodeState,
}

/// Summary of a single tick's work.
#[derive(Debug, Default, Clone, Copy)]
pub struct TickReport {
    pub asserted: usize,
    pub finalized: usize,
}

impl Node {
    /// Connects to the home chain and prepares the node (no transactions sent yet).
    pub async fn connect(cfg: Config) -> Result<Self> {
        let (provider, signer) = build_provider(&cfg.home_rpc_url, &cfg.operator_key)?;
        let account = signer.address;
        let home_chain = retry("get_chain_id", || async {
            Ok(provider.get_chain_id().await?)
        })
        .await?;
        let dest_chain = cfg.dest_chain_id.unwrap_or(home_chain);

        // Asset registry: logical USDC -> the configured collateral token.
        let usdc = keccak256("USDC");
        let registry = AssetRegistry::new().with(
            usdc,
            AssetInfo {
                token: cfg.collateral.into_word(),
                decimals: cfg.collateral_decimals,
                native: false,
            },
        );
        let caps = Capabilities {
            chain: dest_chain,
            finality: FinalityPolicy::new(cfg.finality_confirmations),
            assets: registry.clone(),
        };
        let adapter = EvmAdapter::new(provider.clone(), signer, caps);

        let settlement = ISettlement::new(cfg.settlement, provider.clone());
        let challenge_window = retry("challengeWindow", || async {
            Ok(settlement.challengeWindow().call().await?)
        })
        .await?;

        let handle_node = keccak256(cfg.handle.as_bytes());

        let mut state = NodeState::load(&cfg.state_file)?;
        if state.last_block == 0 {
            let head = retry("head", || async { Ok(provider.get_block_number().await?) }).await?;
            state.last_block = cfg.start_block.unwrap_or(head);
        }

        info!(
            target: "farmore::node",
            %account, home_chain, dest_chain, challenge_window,
            settlement = %cfg.settlement, "node connected"
        );

        Ok(Self {
            cfg,
            provider,
            account,
            adapter,
            dest_chain,
            handle_node,
            registry,
            challenge_window,
            state,
        })
    }

    fn settlement(&self) -> ISettlement::ISettlementInstance<DynProvider> {
        ISettlement::new(self.cfg.settlement, self.provider.clone())
    }

    /// Registers the operator's identity and tops up its collateral bond. Idempotent:
    /// safe to call on every start.
    pub async fn bootstrap(&self) -> Result<()> {
        let ns = INamespace::new(self.cfg.namespace, self.provider.clone());
        let owner = retry("ownerOf", || async {
            Ok(ns.ownerOf(self.handle_node).call().await?)
        })
        .await?;
        if owner == Address::ZERO {
            let r = ns
                .register(self.cfg.handle.clone())
                .send()
                .await?
                .get_receipt()
                .await?;
            if !r.status() {
                bail!("handle registration reverted");
            }
            info!(target: "farmore::node", handle = %self.cfg.handle, "registered identity");
        } else if owner != self.account {
            bail!(
                "handle {} is owned by {owner}, not the operator",
                self.cfg.handle
            );
        }

        let vault = IBondVault::new(self.cfg.bond_vault, self.provider.clone());
        let free = retry("freeBondOf", || async {
            Ok(vault
                .freeBondOf(self.account, self.cfg.collateral)
                .call()
                .await?)
        })
        .await?;
        if free < self.cfg.bond_amount {
            let need = self.cfg.bond_amount - free;
            let token = IERC20Faucet::new(self.cfg.collateral, self.provider.clone());

            if self.cfg.faucet {
                let bal = token.balanceOf(self.account).call().await?;
                if bal < need {
                    let r = token
                        .faucet(self.account, need - bal)
                        .send()
                        .await?
                        .get_receipt()
                        .await?;
                    if !r.status() {
                        bail!("faucet reverted");
                    }
                }
            }
            let allowance = token
                .allowance(self.account, self.cfg.bond_vault)
                .call()
                .await?;
            if allowance < need {
                let r = token
                    .approve(self.cfg.bond_vault, U256::MAX)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;
                if !r.status() {
                    bail!("approve reverted");
                }
            }
            let r = vault
                .deposit(self.cfg.collateral, need)
                .send()
                .await?
                .get_receipt()
                .await?;
            if !r.status() {
                bail!("bond deposit reverted");
            }
            info!(target: "farmore::node", bonded = %need, "topped up bond");
        }

        // Ensure working inventory in the operator wallet for fronting (separate from the
        // bond). On testnet the faucet backs this; on mainnet the operator funds it.
        if self.cfg.faucet && self.cfg.front_inventory > U256::ZERO {
            let token = IERC20Faucet::new(self.cfg.collateral, self.provider.clone());
            let bal = token.balanceOf(self.account).call().await?;
            if bal < self.cfg.front_inventory {
                let r = token
                    .faucet(self.account, self.cfg.front_inventory - bal)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;
                if !r.status() {
                    bail!("inventory faucet reverted");
                }
                info!(target: "farmore::node", inventory = %self.cfg.front_inventory, "topped up front inventory");
            }
        }
        Ok(())
    }

    /// Scans for open intents and, for each one it can serve, fronts the recipient and
    /// asserts fulfilment. Per-order errors are isolated. Returns the number asserted.
    pub async fn scan_once(&mut self) -> Result<usize> {
        let head = retry("head", || async {
            Ok(self.provider.get_block_number().await?)
        })
        .await?;
        let from = self.state.last_block;
        if head < from {
            return Ok(0);
        }

        let settlement = self.settlement();
        let opens = retry("query Open", || async {
            Ok(settlement
                .Open_filter()
                .from_block(from)
                .to_block(head)
                .query()
                .await?)
        })
        .await?;

        let mut asserted = 0usize;
        for (ev, _log) in opens {
            let order_id = ev.orderId;
            match self.try_serve(order_id, &ev.resolvedOrder).await {
                Ok(true) => asserted += 1,
                Ok(false) => {}
                Err(e) => {
                    error!(target: "farmore::node", order = %order_id, error = %e, "failed to serve intent");
                }
            }
        }

        self.state.last_block = head + 1;
        self.persist()?;
        Ok(asserted)
    }

    /// Attempts to serve one open intent. Returns Ok(true) if it asserted, Ok(false) if
    /// it deliberately skipped. Idempotent w.r.t. the journal and the on-chain status.
    async fn try_serve(
        &mut self,
        order_id: B256,
        ro: &ISettlement::ResolvedCrossChainOrder,
    ) -> Result<bool> {
        let key = order_id.to_string();

        // Resume / dedupe from the journal.
        let mut fill_hash: Option<B256> = None;
        if let Some(rec) = self.state.get(&key) {
            match rec.stage {
                Stage::Asserted | Stage::Finalized | Stage::Skipped => return Ok(false),
                Stage::Fronted => {
                    fill_hash = rec.fill_hash.as_ref().and_then(|h| h.parse().ok());
                }
            }
        }

        // What does the recipient receive?
        let out = ro
            .minReceived
            .first()
            .ok_or_else(|| anyhow!("intent has no minReceived output"))?;
        if out.chainId != U256::from(self.dest_chain) {
            self.mark(&key, Stage::Skipped, None, 0)?;
            return Ok(false);
        }
        let Some((asset, _info)) = self.registry.resolve_by_token(&out.token) else {
            self.mark(&key, Stage::Skipped, None, 0)?;
            return Ok(false);
        };
        if !self.adapter.supports(self.dest_chain, &asset) {
            self.mark(&key, Stage::Skipped, None, 0)?;
            return Ok(false);
        }

        // Only act on intents still open on chain.
        let settlement = self.settlement();
        let s = settlement.statusOf(order_id).call().await?;
        if s != status::OPENED {
            self.mark(&key, Stage::Skipped, None, 0)?;
            return Ok(false);
        }

        // Front the recipient from the operator's own funds (unless resuming).
        let fill = match fill_hash {
            Some(h) => h,
            None => {
                let receipt = self
                    .adapter
                    .transfer(&TransferRequest {
                        asset,
                        to: out.recipient,
                        amount: out.amount,
                    })
                    .await
                    .context("fronting recipient")?;
                self.mark(&key, Stage::Fronted, Some(receipt.tx), 0)?;
                info!(target: "farmore::node", order = %order_id, fill = %receipt.tx, amount = %out.amount, "fronted recipient");
                receipt.tx
            }
        };

        // Assert fulfilment on the home chain with bond at stake.
        let r = settlement
            .assertFulfillment(
                order_id,
                self.handle_node,
                self.cfg.collateral,
                self.cfg.bond_amount,
                fill,
            )
            .send()
            .await?
            .get_receipt()
            .await?;
        if !r.status() {
            bail!("assertFulfillment reverted");
        }
        let finalizable = settlement.finalizableAt(order_id).call().await?;
        self.mark(&key, Stage::Asserted, Some(fill), finalizable)?;
        info!(target: "farmore::node", order = %order_id, finalizable_at = finalizable, "asserted fulfilment");
        Ok(true)
    }

    /// Finalizes every asserted order whose challenge window has elapsed and that remains
    /// unchallenged. Returns the number finalized.
    pub async fn finalize_due(&mut self) -> Result<usize> {
        let now = self.block_timestamp().await?;
        let settlement = self.settlement();

        let due: Vec<(String, u64)> = self
            .state
            .orders
            .iter()
            .filter(|(_, r)| r.stage == Stage::Asserted)
            .map(|(k, r)| (k.clone(), r.finalizable_at))
            .collect();

        let mut finalized = 0usize;
        for (key, finalizable_at) in due {
            if now < finalizable_at {
                continue;
            }
            let order_id: B256 = match key.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            match self.try_finalize(&settlement, order_id, &key).await {
                Ok(true) => finalized += 1,
                Ok(false) => {}
                Err(e) => {
                    error!(target: "farmore::node", order = %order_id, error = %e, "finalize failed")
                }
            }
        }
        Ok(finalized)
    }

    async fn try_finalize(
        &mut self,
        settlement: &ISettlement::ISettlementInstance<DynProvider>,
        order_id: B256,
        key: &str,
    ) -> Result<bool> {
        let s = settlement.statusOf(order_id).call().await?;
        match s {
            status::ASSERTED => {
                let r = settlement
                    .finalize(order_id)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;
                if !r.status() {
                    bail!("finalize reverted");
                }
                self.mark(
                    key,
                    Stage::Finalized,
                    self.fill_of(key),
                    self.finalizable_of(key),
                )?;
                info!(target: "farmore::node", order = %order_id, "finalized — FAR minted under fairness rules");
                Ok(true)
            }
            status::FINALIZED => {
                self.mark(
                    key,
                    Stage::Finalized,
                    self.fill_of(key),
                    self.finalizable_of(key),
                )?;
                Ok(false)
            }
            status::SLASHED => {
                self.mark(
                    key,
                    Stage::Skipped,
                    self.fill_of(key),
                    self.finalizable_of(key),
                )?;
                warn!(target: "farmore::node", order = %order_id, "assertion was slashed; no mint");
                Ok(false)
            }
            // Disputed or otherwise pending: leave for a later tick.
            _ => Ok(false),
        }
    }

    /// One full tick: scan/front/assert, then finalize due orders.
    pub async fn tick(&mut self) -> Result<TickReport> {
        let asserted = self.scan_once().await?;
        let finalized = self.finalize_due().await?;
        Ok(TickReport {
            asserted,
            finalized,
        })
    }

    /// Runs the node loop until SIGINT, ticking every `poll_ms`.
    pub async fn run(mut self) -> Result<()> {
        self.bootstrap().await.context("bootstrap")?;
        let mut ticker = tokio::time::interval(Duration::from_millis(self.cfg.poll_ms));
        info!(target: "farmore::node", poll_ms = self.cfg.poll_ms, "node running");
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!(target: "farmore::node", "shutdown signal received");
                    self.persist().ok();
                    return Ok(());
                }
                _ = ticker.tick() => {
                    match self.tick().await {
                        Ok(r) if r.asserted > 0 || r.finalized > 0 => {
                            info!(target: "farmore::node", asserted = r.asserted, finalized = r.finalized, "tick");
                        }
                        Ok(_) => {}
                        Err(e) => error!(target: "farmore::node", error = %e, "tick error; continuing"),
                    }
                }
            }
        }
    }

    // --- accessors used by tests/operators ---
    pub fn account(&self) -> Address {
        self.account
    }
    pub fn handle_node(&self) -> B256 {
        self.handle_node
    }
    pub fn challenge_window(&self) -> u64 {
        self.challenge_window
    }
    pub fn provider(&self) -> &DynProvider {
        &self.provider
    }

    // --- helpers ---

    async fn block_timestamp(&self) -> Result<u64> {
        let block = retry("get_block", || async {
            self.provider
                .get_block(BlockId::latest())
                .await?
                .ok_or_else(|| anyhow!("no latest block"))
        })
        .await?;
        Ok(block.header.timestamp)
    }

    fn fill_of(&self, key: &str) -> Option<B256> {
        self.state
            .get(key)
            .and_then(|r| r.fill_hash.as_ref())
            .and_then(|h| h.parse().ok())
    }
    fn finalizable_of(&self, key: &str) -> u64 {
        self.state.get(key).map(|r| r.finalizable_at).unwrap_or(0)
    }

    fn mark(
        &mut self,
        key: &str,
        stage: Stage,
        fill_hash: Option<B256>,
        finalizable_at: u64,
    ) -> Result<()> {
        self.state.set(
            key.to_string(),
            OrderRecord {
                stage,
                fill_hash: fill_hash.map(|h| h.to_string()),
                finalizable_at,
            },
        );
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        self.state.save(&self.cfg.state_file)
    }
}
