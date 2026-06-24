//! End-to-end integration test of the full Farmore loop on a local anvil.
//!
//! It spawns anvil, deploys the real contracts with the real Foundry deploy script,
//! runs a Farmore Node that bonds and serves intents, and asserts both money paths:
//!
//! - HAPPY: node fronts the recipient (who actually receives funds on the destination),
//!   asserts, the window elapses, the node finalizes, and FAR is minted to the solver
//!   under the fairness rules (bond == reference => reward == base).
//! - SLASH: node fronts + asserts, a challenger disputes, the arbiter rules for the
//!   challenger, the bond is slashed and NOTHING is minted.
//!
//! Requires `forge` and `anvil` on PATH.

use std::path::PathBuf;
use std::process::Command;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::providers::ext::AnvilApi;
use alloy::providers::Provider;
use alloy::sol_types::SolValue;
use farmore_ethereum_adapter::{build_provider, IERC20Faucet, ISettlement};
use farmore_node::config::Config;
use farmore_node::Node;

// Standard deterministic anvil accounts (public test keys — never used with real funds).
const DEPLOYER_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const DEPLOYER_ADDR: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
const OPERATOR_KEY: &str = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const OPERATOR_ADDR: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const USER_KEY: &str = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
const RECIPIENT_ADDR: &str = "0x90F79bf6EB2c4f870365E785982E1f101E93b906";
const CHALLENGER_KEY: &str = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";

#[derive(serde::Deserialize)]
struct Deployment {
    settlement: Address,
    far: Address,
    namespace: Address,
    #[serde(rename = "bondVault")]
    bond_vault: Address,
    collateral: Address,
}

fn contracts_dir() -> PathBuf {
    // Pinned farmore-contracts location: prefer the env override (set by CI / the
    // Makefile), else the sibling repo checked out next to farmore-node.
    if let Ok(dir) = std::env::var("FARMORE_CONTRACTS_DIR") {
        return PathBuf::from(dir)
            .canonicalize()
            .expect("FARMORE_CONTRACTS_DIR does not exist");
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../farmore-contracts")
        .canonicalize()
        .expect("check out farmore-contracts as a sibling or set FARMORE_CONTRACTS_DIR")
}

fn deploy(rpc: &str) -> Deployment {
    let dir = contracts_dir();
    let status = Command::new("forge")
        .args([
            "script",
            "script/Deploy.s.sol:Deploy",
            "--rpc-url",
            rpc,
            "--broadcast",
        ])
        .current_dir(&dir)
        .env("DEPLOYER_PRIVATE_KEY", DEPLOYER_KEY)
        .env("FARMORE_ARBITER", DEPLOYER_ADDR)
        .env("FARMORE_TREASURY", DEPLOYER_ADDR)
        .status()
        .expect("run forge script (is forge on PATH?)");
    assert!(status.success(), "forge deploy failed");
    let json = std::fs::read(dir.join("deployments/31337.json")).expect("read deployments");
    serde_json::from_slice(&json).expect("parse deployments")
}

/// Opens an intent paying `amount` of the collateral to `recipient`, returns its orderId.
async fn open_intent(
    rpc: &str,
    settlement: Address,
    collateral: Address,
    recipient: Address,
    amount: U256,
) -> B256 {
    let (provider, _signer) = build_provider(rpc, USER_KEY).unwrap();
    let s = ISettlement::new(settlement, provider.clone());
    let now = provider
        .get_block(alloy::eips::BlockId::latest())
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    let fod = ISettlement::FarmoreOrderData {
        recipientHandle: keccak256("recipient"),
        asset: keccak256("USDC"),
        amount,
        destinationChainId: 31337,
        destinationAsset: collateral.into_word(),
        recipientAddress: recipient.into_word(),
    };
    let order = ISettlement::OnchainCrossChainOrder {
        fillDeadline: (now + 3600) as u32,
        orderDataType: keccak256("FarmoreOrderData"),
        orderData: Bytes::from(fod.abi_encode()),
    };
    let receipt = s
        .openIntent(order)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status());
    for log in receipt.inner.logs() {
        if let Ok(d) = log.log_decode::<ISettlement::IntentOpened>() {
            return d.inner.data.orderId;
        }
    }
    panic!("no IntentOpened event");
}

fn node_config(rpc: &str, d: &Deployment, handle: &str, state_file: PathBuf) -> Config {
    Config {
        home_rpc_url: rpc.to_string(),
        operator_key: OPERATOR_KEY.to_string(),
        settlement: d.settlement,
        namespace: d.namespace,
        bond_vault: d.bond_vault,
        collateral: d.collateral,
        far: d.far,
        handle: handle.to_string(),
        bond_amount: U256::from(1_000_000_000u64), // 1,000 tUSD == reference => reward == base == 100 FAR
        front_inventory: U256::from(10_000_000_000u64), // 10,000 tUSD working inventory
        dest_chain_id: Some(31337),
        collateral_decimals: 6,
        finality_confirmations: 1,
        poll_ms: 250,
        start_block: None,
        state_file: state_file.to_string_lossy().to_string(),
        faucet: true,
    }
}

async fn far_balance(rpc: &str, far: Address, who: Address) -> U256 {
    let (p, _) = build_provider(rpc, OPERATOR_KEY).unwrap();
    IERC20Faucet::new(far, p)
        .balanceOf(who)
        .call()
        .await
        .unwrap()
}

async fn erc20_balance(rpc: &str, token: Address, who: Address) -> U256 {
    let (p, _) = build_provider(rpc, OPERATOR_KEY).unwrap();
    IERC20Faucet::new(token, p)
        .balanceOf(who)
        .call()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_loop_mints_and_slash_path() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("info,farmore=debug")
        .try_init();

    let anvil = alloy::node_bindings::Anvil::new().chain_id(31337).spawn();
    let rpc = anvil.endpoint();
    let d = deploy(&rpc);

    let operator: Address = OPERATOR_ADDR.parse().unwrap();
    let recipient: Address = RECIPIENT_ADDR.parse().unwrap();
    let amount = U256::from(100_000_000u64); // 100 tUSD

    let state_file = std::env::temp_dir().join(format!("farmore-e2e-{}.json", anvil.port()));
    let _ = std::fs::remove_file(&state_file);

    let mut node = Node::connect(node_config(&rpc, &d, "operator_node", state_file.clone()))
        .await
        .unwrap();
    node.bootstrap().await.unwrap();

    // ---------------- HAPPY PATH ----------------
    let recip_before = erc20_balance(&rpc, d.collateral, recipient).await;
    let order_a = open_intent(&rpc, d.settlement, d.collateral, recipient, amount).await;

    let asserted = node.scan_once().await.unwrap();
    assert_eq!(asserted, 1, "node should have fronted + asserted intent A");

    // Recipient actually received the funds on the destination.
    let recip_after = erc20_balance(&rpc, d.collateral, recipient).await;
    assert_eq!(
        recip_after - recip_before,
        amount,
        "recipient must be fronted"
    );

    // Status is Asserted (2).
    let s = ISettlement::new(d.settlement, node.provider().clone());
    assert_eq!(s.statusOf(order_a).call().await.unwrap(), 2u8);

    // Elapse the challenge window and finalize.
    node.provider().anvil_increase_time(120u64).await.unwrap();
    node.provider().anvil_mine(Some(1u64), None).await.unwrap();
    let finalized = node.finalize_due().await.unwrap();
    assert_eq!(finalized, 1, "node should finalize intent A");

    assert_eq!(
        s.statusOf(order_a).call().await.unwrap(),
        4u8,
        "A finalized"
    );
    let far_after_a = far_balance(&rpc, d.far, operator).await;
    assert_eq!(
        far_after_a,
        U256::from(100_000_000_000_000_000_000u128),
        "100 FAR minted (base reward)"
    );

    // ---------------- SLASH PATH ----------------
    let order_b = open_intent(&rpc, d.settlement, d.collateral, recipient, amount).await;
    let asserted_b = node.scan_once().await.unwrap();
    assert_eq!(asserted_b, 1, "node should front + assert intent B");
    assert_eq!(s.statusOf(order_b).call().await.unwrap(), 2u8);

    // A challenger disputes within the window (dispute bond = 10% of 1,000 tUSD = 100 tUSD).
    let (cp, challenger_signer) = build_provider(&rpc, CHALLENGER_KEY).unwrap();
    let dispute_bond = U256::from(100_000_000u64);
    let ctoken = IERC20Faucet::new(d.collateral, cp.clone());
    ctoken
        .faucet(challenger_signer.address, dispute_bond)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    ctoken
        .approve(d.settlement, dispute_bond)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    let cs = ISettlement::new(d.settlement, cp);
    cs.dispute(order_b)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert_eq!(s.statusOf(order_b).call().await.unwrap(), 3u8, "B disputed");

    // The arbiter (deployer) rules for the challenger -> slash, no mint.
    let (ap, _) = build_provider(&rpc, DEPLOYER_KEY).unwrap();
    ISettlement::new(d.settlement, ap)
        .resolveDispute(order_b, true)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert_eq!(s.statusOf(order_b).call().await.unwrap(), 5u8, "B slashed");

    // The node attempts to finalize due orders; B must mint nothing.
    node.provider().anvil_increase_time(120u64).await.unwrap();
    node.provider().anvil_mine(Some(1u64), None).await.unwrap();
    let _ = node.finalize_due().await.unwrap();

    let far_after_b = far_balance(&rpc, d.far, operator).await;
    assert_eq!(far_after_b, far_after_a, "slash path must not mint any FAR");

    let _ = std::fs::remove_file(&state_file);
}
