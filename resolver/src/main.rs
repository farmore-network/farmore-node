//! # farmore-resolver
//!
//! The Farmore resolver/indexer: a real HTTP service that reads the on-chain Namespace
//! and exposes handle resolution, send-to-handle order preparation, and sign-in based on
//! handle ownership (an SIWE-style challenge verified against the handle's owner).
//!
//! Endpoints:
//!   GET  /health                      liveness + chain id
//!   GET  /resolve/:handle             account record for a handle
//!   GET  /resolve/:handle/:asset      receive destination for a handle + asset
//!   POST /send                        build an ERC-7683 order to send to a handle
//!   GET  /signin/:handle/nonce        issue a sign-in challenge
//!   POST /signin/verify               verify a signed challenge against handle ownership
//!
//! Config (env): FARMORE_HOME_RPC_URL, FARMORE_NAMESPACE, FARMORE_SETTLEMENT,
//!               FARMORE_COLLATERAL, RESOLVER_BIND (default 0.0.0.0:8080).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{keccak256, Address, Signature, B256, U256};
use alloy::providers::{DynProvider, Provider};
use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use farmore_ethereum_adapter::{build_readonly_provider, INamespace};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    provider: DynProvider,
    namespace: Address,
    collateral: Address,
    settlement: Address,
    chain_id: u64,
    /// handle -> issued sign-in nonce (in-memory; a production deployment uses a store).
    nonces: Arc<Mutex<HashMap<String, String>>>,
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing env var {key}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let rpc = env("FARMORE_HOME_RPC_URL")?;
    let provider = build_readonly_provider(&rpc)?;
    let chain_id = provider.get_chain_id().await.context("get_chain_id")?;
    let state = AppState {
        provider,
        namespace: env("FARMORE_NAMESPACE")?
            .parse()
            .context("FARMORE_NAMESPACE")?,
        collateral: env("FARMORE_COLLATERAL")?
            .parse()
            .context("FARMORE_COLLATERAL")?,
        settlement: env("FARMORE_SETTLEMENT")?
            .parse()
            .context("FARMORE_SETTLEMENT")?,
        chain_id,
        nonces: Arc::new(Mutex::new(HashMap::new())),
    };

    let bind = std::env::var("RESOLVER_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let app = Router::new()
        .route("/health", get(health))
        .route("/resolve/:handle", get(resolve))
        .route("/resolve/:handle/:asset", get(resolve_asset))
        .route("/send", post(prepare_send))
        .route("/signin/:handle/nonce", get(signin_nonce))
        .route("/signin/verify", post(signin_verify))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    info!(target: "farmore::resolver", %bind, chain_id, "resolver listening");
    axum::serve(listener, app).await.context("serve")?;
    Ok(())
}

type ApiResult =
    std::result::Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

fn bad(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(json!({ "error": msg.into() })))
}

async fn health(State(s): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "chainId": s.chain_id,
        "namespace": s.namespace.to_string(),
        "settlement": s.settlement.to_string(),
    }))
}

async fn resolve(State(s): State<AppState>, Path(handle): Path<String>) -> ApiResult {
    let ns = INamespace::new(s.namespace, s.provider.clone());
    let node = ns
        .nodeOf(handle.clone())
        .call()
        .await
        .map_err(|_| bad(StatusCode::BAD_REQUEST, "invalid handle"))?;
    let acct = ns
        .resolve(handle.clone())
        .call()
        .await
        .map_err(|_| bad(StatusCode::NOT_FOUND, "handle not registered"))?;
    Ok(Json(json!({
        "handle": handle,
        "node": node.to_string(),
        "owner": acct.owner.to_string(),
        "defaultChainId": acct.defaultChainId,
        "defaultAddress": acct.defaultAddress.to_string(),
    })))
}

async fn resolve_asset(
    State(s): State<AppState>,
    Path((handle, asset)): Path<(String, String)>,
) -> ApiResult {
    let ns = INamespace::new(s.namespace, s.provider.clone());
    let acct = ns
        .resolve(handle.clone())
        .call()
        .await
        .map_err(|_| bad(StatusCode::NOT_FOUND, "handle not registered"))?;
    Ok(Json(json!({
        "handle": handle,
        "asset": asset,
        "assetId": parse_asset(&asset).to_string(),
        "chainId": acct.defaultChainId,
        "address": acct.defaultAddress.to_string(),
    })))
}

fn parse_asset(asset: &str) -> B256 {
    if let Some(hex) = asset.strip_prefix("0x") {
        if let Ok(b) = hex.parse::<B256>() {
            return b;
        }
    }
    keccak256(asset.as_bytes())
}

#[derive(Deserialize)]
struct SendRequest {
    to_handle: String,
    asset: String,
    /// Amount in the asset's base units, decimal string.
    amount: String,
    #[serde(default)]
    fill_deadline_secs: Option<u64>,
}

#[derive(Serialize)]
struct PreparedOrder {
    to_handle: String,
    recipient: String,
    destination_chain_id: u64,
    destination_asset: String,
    asset_id: String,
    amount: String,
    fill_deadline: u64,
    order_data_type: String,
    settlement: String,
}

/// Prepares an ERC-7683 OnchainCrossChainOrder to pay a handle. The caller signs and
/// submits `openIntent` with this data; the resolver does no custody.
async fn prepare_send(State(s): State<AppState>, Json(req): Json<SendRequest>) -> ApiResult {
    let ns = INamespace::new(s.namespace, s.provider.clone());
    let acct = ns
        .resolve(req.to_handle.clone())
        .call()
        .await
        .map_err(|_| bad(StatusCode::NOT_FOUND, "handle not registered"))?;
    let amount: U256 = req
        .amount
        .parse()
        .map_err(|_| bad(StatusCode::BAD_REQUEST, "invalid amount"))?;
    let now = unix_now();
    let fill_deadline = now + req.fill_deadline_secs.unwrap_or(3600);

    let prepared = PreparedOrder {
        to_handle: req.to_handle,
        recipient: acct.defaultAddress.to_string(),
        destination_chain_id: acct.defaultChainId,
        destination_asset: s.collateral.into_word().to_string(),
        asset_id: parse_asset(&req.asset).to_string(),
        amount: amount.to_string(),
        fill_deadline,
        order_data_type: keccak256("FarmoreOrderData").to_string(),
        settlement: s.settlement.to_string(),
    };
    match serde_json::to_value(prepared) {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(bad(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("serialize: {e}"),
        )),
    }
}

async fn signin_nonce(State(s): State<AppState>, Path(handle): Path<String>) -> ApiResult {
    let n = unix_now();
    let nonce = format!("farmore-signin-{handle}-{n}");
    s.nonces
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(handle.clone(), nonce.clone());
    let message = signin_message(&handle, &nonce, s.chain_id);
    Ok(Json(
        json!({ "handle": handle, "nonce": nonce, "message": message }),
    ))
}

#[derive(Deserialize)]
struct SignInVerify {
    handle: String,
    signature: String,
}

/// Verifies a sign-in: recovers the signer of the issued challenge and checks it owns the
/// handle on chain. Returns a minimal session assertion on success.
async fn signin_verify(State(s): State<AppState>, Json(req): Json<SignInVerify>) -> ApiResult {
    let nonce = s
        .nonces
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&req.handle)
        .cloned()
        .ok_or_else(|| {
            bad(
                StatusCode::BAD_REQUEST,
                "no active nonce; request one first",
            )
        })?;
    let message = signin_message(&req.handle, &nonce, s.chain_id);

    let sig: Signature = req
        .signature
        .parse()
        .map_err(|_| bad(StatusCode::BAD_REQUEST, "invalid signature"))?;
    let recovered = sig
        .recover_address_from_msg(message.as_bytes())
        .map_err(|_| bad(StatusCode::BAD_REQUEST, "could not recover signer"))?;

    let ns = INamespace::new(s.namespace, s.provider.clone());
    let node = ns
        .nodeOf(req.handle.clone())
        .call()
        .await
        .map_err(|_| bad(StatusCode::BAD_REQUEST, "invalid handle"))?;
    let owner = ns
        .ownerOf(node)
        .call()
        .await
        .map_err(|e| bad(StatusCode::INTERNAL_SERVER_ERROR, format!("rpc: {e}")))?;

    if owner == Address::ZERO {
        return Err(bad(StatusCode::NOT_FOUND, "handle not registered"));
    }
    if recovered != owner {
        return Err(bad(
            StatusCode::UNAUTHORIZED,
            "signer does not own this handle",
        ));
    }
    s.nonces
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&req.handle);
    info!(target: "farmore::resolver", handle = %req.handle, owner = %owner, "sign-in verified");
    Ok(Json(
        json!({ "authenticated": true, "handle": req.handle, "owner": owner.to_string() }),
    ))
}

fn signin_message(handle: &str, nonce: &str, chain_id: u64) -> String {
    format!("Farmore sign-in\nhandle: {handle}\nchainId: {chain_id}\nnonce: {nonce}")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
