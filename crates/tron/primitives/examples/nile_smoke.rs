//! Manual smoke test against Nile testnet.
//!
//! Usage:
//!   cargo run --example nile_smoke -- address           # print address for faucet
//!   cargo run --example nile_smoke -- send <to_T_addr>  # self-transfer 1 TRX
//!
//! Requires TRON_PRIVATE_KEY env var (hex, no 0x).

use alloy_signer_local::PrivateKeySigner;
use foundry_tron_primitives::{proto, sign, tapos, to_base58};
use prost::Message;
use std::str::FromStr;

const NILE: &str = "https://nile.trongrid.io";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let signer = PrivateKeySigner::from_str(&std::env::var("TRON_PRIVATE_KEY")?)?;

    match args.get(1).map(String::as_str) {
        Some("address") => {
            println!("{}", to_base58(signer.address()));
        }
        Some("send") => {
            let to = foundry_tron_primitives::parse_address(&args[2])?;
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
        _ => eprintln!("usage: nile_smoke address | send <to>"),
    }
    Ok(())
}
