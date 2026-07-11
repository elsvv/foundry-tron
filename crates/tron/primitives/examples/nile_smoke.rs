//! Manual smoke test against Nile testnet. Subcommands:
//!
//!   cargo run --example nile_smoke -- keygen
//!   cargo run --example nile_smoke -- address
//!   cargo run --example nile_smoke -- send <to_T_addr>
//!
//! `keygen` prints a throwaway recipient key + T-address. `address` prints the
//! funded-key T-address for the faucet. `send` transfers 1 TRX to a DIFFERENT
//! address (see the note below). `address` and `send` require the
//! TRON_PRIVATE_KEY env var (hex, no 0x).
//!
//! Note: java-tron's TransferActuator rejects a transfer whose recipient equals
//! the owner ("Cannot transfer TRX to yourself"), so `<to_T_addr>` must differ
//! from the funded key. Use `keygen` to mint a throwaway recipient; it does not
//! need to be funded to receive TRX.

// Standalone manual smoke-test binary: plain stdout/stderr is the intended
// output here, so the workspace `sh_*`-macro rule (for CLI crates) does not
// apply. Same rationale as the sibling `foundry-tron-provider` test gate.
#![allow(clippy::disallowed_macros)]

use alloy_signer_local::PrivateKeySigner;
use foundry_tron_primitives::{proto, sign, tapos, to_base58};
use prost::Message;
use std::str::FromStr;

const NILE: &str = "https://nile.trongrid.io";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("keygen") => {
            // Fresh throwaway key to use as a distinct `send` recipient.
            let recipient = PrivateKeySigner::random();
            println!(
                "private_key: {}",
                alloy_primitives::hex::encode(recipient.credential().to_bytes())
            );
            println!("address:     {}", to_base58(recipient.address()));
        }
        Some("address") => {
            let signer = PrivateKeySigner::from_str(&std::env::var("TRON_PRIVATE_KEY")?)?;
            println!("{}", to_base58(signer.address()));
        }
        Some("send") => {
            let signer = PrivateKeySigner::from_str(&std::env::var("TRON_PRIVATE_KEY")?)?;
            let to = foundry_tron_primitives::parse_address(&args[2])?;
            // java-tron rejects owner == to ("Cannot transfer TRX to yourself"),
            // so refuse the self-transfer up front instead of eating a
            // CONTRACT_VALIDATE_ERROR from the node. Run `keygen` for a
            // throwaway recipient address.
            if to == signer.address() {
                return Err("recipient must differ from the funded key (java-tron rejects self-transfers); run `keygen` for a throwaway address".into());
            }
            // 1. TAPOS from a fresh block.
            let block: serde_json::Value =
                ureq::post(&format!("{NILE}/wallet/getnowblock")).call()?.into_json()?;
            let number = block["block_header"]["raw_data"]["number"].as_i64().unwrap();
            let block_id: [u8; 32] =
                alloy_primitives::hex::decode(block["blockID"].as_str().unwrap())?
                    .try_into()
                    .unwrap();
            let rb = tapos::ref_block(number, &block_id);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as i64;

            // 2. TransferContract of 1 TRX.
            let mut owner21 = vec![0x41];
            owner21.extend_from_slice(signer.address().as_slice());
            let mut to21 = vec![0x41];
            to21.extend_from_slice(to.as_slice());
            let transfer = proto::TransferContract {
                owner_address: owner21,
                to_address: to21,
                amount: 1_000_000,
            };
            let raw = proto::TransactionRaw {
                ref_block_bytes: rb.bytes,
                ref_block_hash: rb.hash,
                expiration: now_ms + 60_000,
                timestamp: now_ms,
                contract: vec![proto::Contract {
                    r#type: proto::ContractType::TransferContract as i32,
                    parameter: Some(prost_types::Any {
                        type_url: proto::type_url(proto::ContractType::TransferContract)
                            .to_string(),
                        value: transfer.encode_to_vec(),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            };

            // 3. Sign and broadcast.
            let signed = sign::sign_raw(raw, &signer)?;
            println!("txid: {}", signed.txid);
            let resp: serde_json::Value = ureq::post(&format!("{NILE}/wallet/broadcasthex"))
                .send_json(serde_json::json!({ "transaction": signed.broadcast_hex() }))?
                .into_json()?;
            println!("broadcast response: {resp}");
        }
        _ => eprintln!("usage: nile_smoke keygen | address | send <to>"),
    }
    Ok(())
}
