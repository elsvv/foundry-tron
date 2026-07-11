//! Typed async client for the Tron wallet HTTP API.

use alloy_primitives::{Address, B256, hex};
use foundry_tron_primitives::{
    address::to_hex41,
    proto::{self, ContractType},
    sign::{SignedTronTx, sign_raw},
    tapos::{RefBlock, ref_block},
};
use prost::Message;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum TronError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("node error {code}: {message}")]
    Api { code: String, message: String },
    #[error("decode error: {0}")]
    Decode(String),
    #[error("timeout: {0}")]
    Timeout(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowBlock {
    pub block_id: [u8; 32],
    pub number: i64,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxInfo {
    pub block_number: i64,
    pub fee_sun: u64,
    pub energy_used: u64,
    pub success: bool,
    pub contract_address: Option<Address>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstantResult {
    pub result: Vec<u8>,
    pub energy_used: u64,
    pub success: bool,
}

pub struct TronProvider {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl TronProvider {
    pub fn new(base_url: &str) -> Result<Self, TronError> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: None,
            client: reqwest::Client::new(),
        })
    }

    pub fn with_api_key(mut self, key: String) -> Self {
        self.api_key = Some(key);
        self
    }

    async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, TronError> {
        let mut req = self.client.post(format!("{}{path}", self.base_url)).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("TRON-PRO-API-KEY", key);
        }
        let value: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
        // The node reports errors in a 200 body: {"Error": "..."} or
        // {"code": "...", "message": "<hex>"}.
        if let Some(err) = value.get("Error").and_then(|e| e.as_str()) {
            return Err(TronError::Api { code: "Error".into(), message: err.to_string() });
        }
        serde_json::from_value(value).map_err(|e| TronError::Decode(e.to_string()))
    }

    pub async fn get_now_block(&self) -> Result<NowBlock, TronError> {
        let v: serde_json::Value =
            self.post_json("/wallet/getnowblock", serde_json::json!({})).await?;
        parse_now_block(&v)
    }

    pub async fn tapos(&self) -> Result<(RefBlock, i64), TronError> {
        let nb = self.get_now_block().await?;
        Ok((ref_block(nb.number, &nb.block_id), nb.timestamp_ms))
    }

    /// Returns the account balance in SUN. A non-existent account (the node
    /// replies with an empty `{}` body) reads as a zero balance.
    pub async fn get_balance(&self, addr: Address) -> Result<u64, TronError> {
        let body = serde_json::json!({ "address": to_hex41(addr), "visible": false });
        let v: serde_json::Value = self.post_json("/wallet/getaccount", body).await?;
        parse_account_balance(&v)
    }

    /// Returns transaction info once the transaction is in a block, or `None`
    /// while it is still pending (the node replies with an empty `{}` body).
    pub async fn get_transaction_info(&self, txid: B256) -> Result<Option<TxInfo>, TronError> {
        let body = serde_json::json!({ "value": hex::encode(txid) });
        let v: serde_json::Value = self.post_json("/wallet/gettransactioninfobyid", body).await?;
        parse_tx_info(&v)
    }

    /// Executes a read-only (constant) contract call and returns its ABI-encoded
    /// output together with the energy estimate. The `data` payload must already
    /// be ABI-encoded (selector + arguments); ABI encoding is the caller's job.
    pub async fn trigger_constant(
        &self,
        owner: Address,
        contract: Address,
        data: &[u8],
    ) -> Result<ConstantResult, TronError> {
        let body = serde_json::json!({
            "owner_address": to_hex41(owner),
            "contract_address": to_hex41(contract),
            "data": hex::encode(data),
            "visible": false,
        });
        let v: serde_json::Value = self.post_json("/wallet/triggerconstantcontract", body).await?;
        parse_constant_result(&v)
    }

    /// Broadcasts a signed transaction via `/wallet/broadcasthex`. On rejection
    /// the node reports `result != true`; the returned `TronError::Api` carries
    /// its `code` and the hex-decoded `message`.
    pub async fn broadcast(&self, tx: &SignedTronTx) -> Result<(), TronError> {
        let body = serde_json::json!({ "transaction": tx.broadcast_hex() });
        let v: serde_json::Value = self.post_json("/wallet/broadcasthex", body).await?;
        parse_broadcast_result(&v)
    }

    /// Polls `get_transaction_info` until the transaction lands in a block, up to
    /// `max_attempts` times spaced by `interval`. The bound is mandatory: an
    /// unbounded poll (the TronBox lesson) can hang forever on a dropped tx.
    pub async fn wait_for_confirmation(
        &self,
        txid: B256,
        max_attempts: u32,
        interval: std::time::Duration,
    ) -> Result<TxInfo, TronError> {
        for _ in 0..max_attempts {
            if let Some(info) = self.get_transaction_info(txid).await? {
                return Ok(info);
            }
            tokio::time::sleep(interval).await;
        }
        Err(TronError::Timeout(format!("tx {txid} not confirmed after {max_attempts} attempts")))
    }

    /// Sends `amount_sun` SUN from `signer`'s account to `to` and waits for
    /// confirmation. Runs the full cycle: fetch TAPOS from a fresh block, build
    /// and sign a `TransferContract` (expiration = block timestamp + 60s),
    /// broadcast it, then poll for confirmation (20 attempts, 3s apart).
    pub async fn send_transfer(
        &self,
        signer: &alloy_signer_local::PrivateKeySigner,
        to: Address,
        amount_sun: i64,
    ) -> Result<(B256, TxInfo), TronError> {
        let (rb, now_ms) = self.tapos().await?;
        let raw = build_transfer_raw(signer.address(), to, amount_sun, rb, now_ms);
        let signed = sign_raw(raw, signer).map_err(|e| TronError::Decode(e.to_string()))?;
        self.broadcast(&signed).await?;
        let info =
            self.wait_for_confirmation(signed.txid, 20, std::time::Duration::from_secs(3)).await?;
        Ok((signed.txid, info))
    }
}

/// Builds the `TransactionRaw` for a native TRX transfer. Deterministic and
/// unit-testable offline: addresses are prefixed with the 0x41 Tron byte and
/// the expiration window is fixed at 60s past `now_ms`.
pub(crate) fn build_transfer_raw(
    owner: Address,
    to: Address,
    amount_sun: i64,
    rb: RefBlock,
    now_ms: i64,
) -> proto::TransactionRaw {
    let addr21 = |a: Address| {
        let mut v = Vec::with_capacity(21);
        v.push(0x41);
        v.extend_from_slice(a.as_slice());
        v
    };
    let transfer = proto::TransferContract {
        owner_address: addr21(owner),
        to_address: addr21(to),
        amount: amount_sun,
    };
    proto::TransactionRaw {
        ref_block_bytes: rb.bytes,
        ref_block_hash: rb.hash,
        expiration: now_ms + 60_000,
        timestamp: now_ms,
        contract: vec![proto::Contract {
            r#type: ContractType::TransferContract as i32,
            parameter: Some(prost_types::Any {
                type_url: proto::type_url(ContractType::TransferContract).to_string(),
                value: transfer.encode_to_vec(),
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Parses a raw `/wallet/getnowblock` JSON response.
pub(crate) fn parse_now_block(v: &serde_json::Value) -> Result<NowBlock, TronError> {
    let missing = |f: &str| TronError::Decode(format!("getnowblock: missing {f}"));
    let block_id_hex = v["blockID"].as_str().ok_or_else(|| missing("blockID"))?;
    let block_id: [u8; 32] = hex::decode(block_id_hex)
        .map_err(|e| TronError::Decode(e.to_string()))?
        .try_into()
        .map_err(|_| TronError::Decode("blockID is not 32 bytes".into()))?;
    let raw = &v["block_header"]["raw_data"];
    Ok(NowBlock {
        block_id,
        number: raw["number"].as_i64().ok_or_else(|| missing("number"))?,
        timestamp_ms: raw["timestamp"].as_i64().ok_or_else(|| missing("timestamp"))?,
    })
}

/// Parses the balance (in SUN) from a `/wallet/getaccount` response. An empty
/// object (unknown account) yields a zero balance.
pub(crate) fn parse_account_balance(v: &serde_json::Value) -> Result<u64, TronError> {
    Ok(v.get("balance").and_then(|b| b.as_u64()).unwrap_or(0))
}

/// Parses a `/wallet/gettransactioninfobyid` response. Returns `None` while the
/// transaction is still pending (empty `{}` body).
pub(crate) fn parse_tx_info(v: &serde_json::Value) -> Result<Option<TxInfo>, TronError> {
    let Some(block_number) = v.get("blockNumber").and_then(|b| b.as_i64()) else {
        return Ok(None);
    };
    let receipt = &v["receipt"];
    let success = match receipt.get("result").and_then(|r| r.as_str()) {
        // `TransferContract` and similar non-VM contracts omit `receipt.result`.
        None => true,
        Some(r) => r == "SUCCESS",
    };
    let contract_address = match v.get("contract_address").and_then(|c| c.as_str()) {
        Some(h41) => Some(
            foundry_tron_primitives::address::parse(h41)
                .map_err(|e| TronError::Decode(e.to_string()))?,
        ),
        None => None,
    };
    Ok(Some(TxInfo {
        block_number,
        fee_sun: v.get("fee").and_then(|f| f.as_u64()).unwrap_or(0),
        energy_used: receipt.get("energy_usage_total").and_then(|e| e.as_u64()).unwrap_or(0),
        success,
        contract_address,
    }))
}

/// Parses a `/wallet/triggerconstantcontract` response. `result` is the raw
/// bytes of `constant_result[0]`, `success` mirrors `result.result`.
pub(crate) fn parse_constant_result(v: &serde_json::Value) -> Result<ConstantResult, TronError> {
    let success = v["result"].get("result").and_then(|r| r.as_bool()).unwrap_or(false);
    let result = match v["constant_result"].get(0).and_then(|r| r.as_str()) {
        Some(h) => hex::decode(h).map_err(|e| TronError::Decode(e.to_string()))?,
        None => Vec::new(),
    };
    Ok(ConstantResult {
        result,
        energy_used: v.get("energy_used").and_then(|e| e.as_u64()).unwrap_or(0),
        success,
    })
}

/// Parses a `/wallet/broadcasthex` response. `result == true` is success;
/// otherwise the node returns a `code` and a hex-encoded `message` which is
/// decoded back to its UTF-8 form (falling back to the raw string).
pub(crate) fn parse_broadcast_result(v: &serde_json::Value) -> Result<(), TronError> {
    if v.get("result").and_then(|r| r.as_bool()) == Some(true) {
        return Ok(());
    }
    let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("UNKNOWN").to_string();
    let raw_msg = v.get("message").and_then(|m| m.as_str()).unwrap_or_default();
    let message = hex::decode(raw_msg)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| raw_msg.to_string());
    Err(TronError::Api { code, message })
}

// `eprintln!` is used to document why live tests self-skip; it is disallowed
// in library code but appropriate for the test gate.
#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../testdata/nile_getnowblock.json")).unwrap()
    }

    #[test]
    fn parses_now_block_fixture() {
        let nb = parse_now_block(&fixture()).unwrap();
        let raw = &fixture()["block_header"]["raw_data"];
        assert_eq!(nb.number, raw["number"].as_i64().unwrap());
        assert_eq!(nb.timestamp_ms, raw["timestamp"].as_i64().unwrap());
        assert_eq!(hex::encode(nb.block_id), fixture()["blockID"].as_str().unwrap());
        assert!(nb.number > 69_000_000);
    }

    #[test]
    fn tapos_fields_derive_from_block() {
        let nb = parse_now_block(&fixture()).unwrap();
        let rb = ref_block(nb.number, &nb.block_id);
        // ref_block_bytes = big-endian(number)[6..8]
        let be = nb.number.to_be_bytes();
        assert_eq!(rb.bytes, be[6..8].to_vec());
        assert_eq!(rb.hash, nb.block_id[8..16].to_vec());
    }

    #[tokio::test]
    async fn live_get_now_block() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        let p = TronProvider::new("https://nile.trongrid.io").unwrap();
        let nb = p.get_now_block().await.unwrap();
        assert!(nb.number > 69_000_000);
        assert_ne!(nb.block_id, [0u8; 32]);
    }

    #[test]
    fn parses_account_balance_fixture() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/nile_getaccount.json")).unwrap();
        let balance = parse_account_balance(&v).unwrap();
        assert_eq!(balance, v["balance"].as_u64().unwrap());
        assert!(balance > 0);
    }

    #[test]
    fn empty_account_is_zero_balance() {
        assert_eq!(parse_account_balance(&serde_json::json!({})).unwrap(), 0);
    }

    #[test]
    fn parses_tx_info_fixture() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/nile_txinfo.json")).unwrap();
        let info = parse_tx_info(&v).unwrap().expect("smoke tx is in a block");
        assert_eq!(info.block_number, 69_090_417);
        assert_eq!(info.fee_sun, 1_100_000);
        assert!(info.success);
        assert_eq!(info.contract_address, None);
    }

    #[test]
    fn pending_tx_info_is_none() {
        assert_eq!(parse_tx_info(&serde_json::json!({})).unwrap(), None);
    }

    #[tokio::test]
    async fn live_balance_and_txinfo() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        let p = TronProvider::new("https://nile.trongrid.io").unwrap();
        let addr =
            foundry_tron_primitives::address::parse("TX7izXWcmofRYonzdcThrS78jifMtVWCuf").unwrap();
        assert!(p.get_balance(addr).await.unwrap() > 0);
        let txid: B256 =
            "3a4d9c5f35b165b1a28f44a57b53cf39b3c2707153e50cdb52d7c0d979750aeb".parse().unwrap();
        let info = p.get_transaction_info(txid).await.unwrap().unwrap();
        assert_eq!(info.block_number, 69_090_417);
    }

    #[test]
    fn parses_constant_result_fixture() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/nile_triggerconstant.json")).unwrap();
        let cr = parse_constant_result(&v).unwrap();
        assert!(cr.success);
        assert_eq!(cr.result.len(), 32, "totalSupply returns uint256");
        assert!(cr.energy_used > 0);
        assert_eq!(hex::encode(&cr.result), v["constant_result"][0].as_str().unwrap());
    }

    #[tokio::test]
    async fn live_trigger_constant_total_supply() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        let p = TronProvider::new("https://nile.trongrid.io").unwrap();
        let owner =
            foundry_tron_primitives::address::parse("TX7izXWcmofRYonzdcThrS78jifMtVWCuf").unwrap();
        // Nile USDT (TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj).
        let usdt =
            foundry_tron_primitives::address::parse("TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj").unwrap();
        // totalSupply() selector.
        let cr = p.trigger_constant(owner, usdt, &hex::decode("18160ddd").unwrap()).await.unwrap();
        assert!(cr.success);
        assert_eq!(cr.result.len(), 32);
        assert!(alloy_primitives::U256::from_be_slice(&cr.result) > alloy_primitives::U256::ZERO);
    }

    #[test]
    fn broadcast_success_parses() {
        let v = serde_json::json!({"code": "SUCCESS", "result": true, "txid": "aa"});
        assert!(parse_broadcast_result(&v).is_ok());
    }

    #[test]
    fn broadcast_error_decodes_hex_message() {
        // The node's real error format: `message` in hex ("SIGERROR" -> hex ascii).
        let v = serde_json::json!({
            "code": "SIGERROR",
            "message": hex::encode("validate signature error"),
        });
        let err = parse_broadcast_result(&v).unwrap_err();
        match err {
            TronError::Api { code, message } => {
                assert_eq!(code, "SIGERROR");
                assert_eq!(message, "validate signature error");
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_confirmation_times_out_on_unknown_tx() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        let p = TronProvider::new("https://nile.trongrid.io").unwrap();
        let bogus = B256::repeat_byte(0xab);
        let res = p.wait_for_confirmation(bogus, 2, std::time::Duration::from_millis(300)).await;
        assert!(matches!(res, Err(TronError::Timeout(_))));
    }

    #[test]
    fn builds_transfer_raw_deterministically() {
        use foundry_tron_primitives::tapos::RefBlock;
        let owner =
            foundry_tron_primitives::address::parse("TX7izXWcmofRYonzdcThrS78jifMtVWCuf").unwrap();
        let to =
            foundry_tron_primitives::address::parse("T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb").unwrap();
        let rb = RefBlock { bytes: vec![0x3c, 0x6f], hash: vec![1, 2, 3, 4, 5, 6, 7, 8] };
        let raw = build_transfer_raw(owner, to, 1_000_000, rb, 1_783_775_034_896);

        assert_eq!(raw.ref_block_bytes, vec![0x3c, 0x6f]);
        assert_eq!(raw.expiration, 1_783_775_034_896 + 60_000);
        assert_eq!(raw.timestamp, 1_783_775_034_896);
        assert_eq!(raw.contract.len(), 1);
        let c = &raw.contract[0];
        assert_eq!(c.r#type, foundry_tron_primitives::proto::ContractType::TransferContract as i32);
        // `parameter` decodes back to the same `TransferContract` with 21-byte
        // 0x41 addresses.
        let tc = <foundry_tron_primitives::proto::TransferContract as prost::Message>::decode(
            c.parameter.as_ref().unwrap().value.as_slice(),
        )
        .unwrap();
        assert_eq!(tc.owner_address[0], 0x41);
        assert_eq!(tc.owner_address[1..], owner.as_slice()[..]);
        assert_eq!(tc.to_address[1..], to.as_slice()[..]);
        assert_eq!(tc.amount, 1_000_000);
    }

    #[tokio::test]
    async fn live_e2e_transfer_on_nile() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        use std::str::FromStr;
        let key = std::env::var("TRON_PRIVATE_KEY").expect("TRON_PRIVATE_KEY for live E2E");
        let signer = alloy_signer_local::PrivateKeySigner::from_str(&key).unwrap();
        let p = TronProvider::new("https://nile.trongrid.io").unwrap();
        // Send 0.1 TRX to the burn address (testnet).
        let to =
            foundry_tron_primitives::address::parse("T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb").unwrap();
        let (txid, info) = p.send_transfer(&signer, to, 100_000).await.unwrap();
        assert!(info.block_number > 69_000_000);
        assert!(info.success);
        eprintln!(
            "live E2E tx: https://nile.tronscan.org/#/transaction/{}",
            alloy_primitives::hex::encode(txid)
        );
    }
}
