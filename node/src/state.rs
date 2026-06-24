//! Crash-safe, idempotent node state. Every step of the loop is journaled to a JSON file
//! that is written atomically (temp file + rename), so a restart resumes exactly where
//! it left off and never re-fronts or double-asserts an order.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The stage an order has reached in this node's pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    /// The recipient has been fronted; assertion not yet confirmed.
    Fronted,
    /// Fulfilment asserted on the home chain; awaiting the challenge window.
    Asserted,
    /// Finalized; reward (if any) observed.
    Finalized,
    /// Deliberately not served by this node (e.g. already taken, unsupported).
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRecord {
    pub stage: Stage,
    pub fill_hash: Option<String>,
    pub finalizable_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeState {
    /// Last home-chain block fully scanned for intents.
    pub last_block: u64,
    /// orderId (0x-hex) -> record.
    pub orders: HashMap<String, OrderRecord>,
}

impl NodeState {
    pub fn load(path: &str) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("corrupt state file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).context("reading state file"),
        }
    }

    /// Persists atomically: write to `<path>.tmp` then rename over `<path>`.
    pub fn save(&self, path: &str) -> Result<()> {
        let tmp = format!("{path}.tmp");
        let bytes = serde_json::to_vec_pretty(self).context("serializing state")?;
        std::fs::write(&tmp, &bytes).context("writing temp state")?;
        std::fs::rename(&tmp, Path::new(path)).context("renaming state file")?;
        Ok(())
    }

    pub fn get(&self, order_id: &str) -> Option<&OrderRecord> {
        self.orders.get(order_id)
    }

    pub fn set(&mut self, order_id: String, record: OrderRecord) {
        self.orders.insert(order_id, record);
    }
}
