//! Typed async client for the Tron wallet HTTP API.

use alloy_primitives::{Address, B256, hex};
use alloy_signer::Signer;
use foundry_tron_primitives::{
    address::{contract_address_from_txid, to_hex41},
    proto::{self, ContractType},
    sign::{SignedTronTx, sign_raw, sign_raw_with},
    tapos::{RefBlock, ref_block},
};
use prost::Message;
use serde::de::DeserializeOwned;
use std::time::Duration;

/// Default `SmartContract.origin_energy_limit` (spec §4.7): the energy budget
/// the deployer bankrolls for callers, in energy units.
pub const DEFAULT_ORIGIN_ENERGY_LIMIT: i64 = 10_000_000;

/// Default `SmartContract.consume_user_resource_percent` (spec §4.7): the share
/// of execution energy paid by the caller rather than the contract (0-100).
pub const DEFAULT_USER_FEE_PERCENT: i64 = 100;

/// Transaction-level knobs shared by every write path: `fee_limit` is the burned
/// cap in SUN (`TransactionRaw.fee_limit`, tag 18) and `expiration_ms` is the
/// validity window past the reference-block timestamp. Defaults mirror the spec:
/// 1000 TRX and 60s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxOptions {
    pub fee_limit: i64,
    pub expiration_ms: i64,
}

impl Default for TxOptions {
    fn default() -> Self {
        Self { fee_limit: 1_000_000_000, expiration_ms: 60_000 }
    }
}

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
    ///
    /// Transient HTTP hiccups (dropped connections, incomplete messages, request
    /// timeouts — routine on public TronGrid) do not abort the wait; they are
    /// treated like a still-pending poll and retried within the same budget. Only
    /// node-level (`Api`) or decode errors propagate immediately.
    pub async fn wait_for_confirmation(
        &self,
        txid: B256,
        max_attempts: u32,
        interval: Duration,
    ) -> Result<TxInfo, TronError> {
        let mut last_http_err = None;
        for _ in 0..max_attempts {
            match self.get_transaction_info(txid).await {
                Ok(Some(info)) => return Ok(info),
                Ok(None) => {}
                Err(e @ TronError::Http(_)) => last_http_err = Some(e),
                Err(e) => return Err(e),
            }
            tokio::time::sleep(interval).await;
        }
        Err(last_http_err.unwrap_or_else(|| {
            TronError::Timeout(format!("tx {txid} not confirmed after {max_attempts} attempts"))
        }))
    }

    /// Sends `amount_sun` SUN from `signer`'s account to `to` and waits for
    /// confirmation. Runs the full cycle: fetch TAPOS from a fresh block, build
    /// and sign a `TransferContract` (default expiration = block timestamp + 60s),
    /// broadcast it, then poll for confirmation (20 attempts, 3s apart).
    pub async fn send_transfer(
        &self,
        signer: &alloy_signer_local::PrivateKeySigner,
        to: Address,
        amount_sun: i64,
    ) -> Result<(B256, TxInfo), TronError> {
        let (rb, now_ms) = self.tapos().await?;
        let raw =
            build_transfer_raw(signer.address(), to, amount_sun, rb, now_ms, &TxOptions::default());
        let signed = sign_raw(raw, signer).map_err(|e| TronError::Decode(e.to_string()))?;
        self.broadcast(&signed).await?;
        let info = self.wait_for_confirmation(signed.txid, 20, Duration::from_secs(3)).await?;
        Ok((signed.txid, info))
    }

    /// Deploys `bytecode_with_args` (creation bytecode ++ constructor args) from
    /// `signer`'s account and waits for confirmation. Generic over any
    /// [`alloy_signer::Signer`] so every wallet backend (keystore, ledger, aws…)
    /// works through the async signing path.
    ///
    /// The deploy address is derived locally from the txid via the java-tron
    /// `WalletUtil` formula ([`contract_address_from_txid`]) — Tron has no EVM
    /// CREATE nonce scheme — and is then cross-checked against the
    /// `contract_address` the node reports once the transaction is mined; a
    /// mismatch is a hard error. Returns `(txid, deploy_address, info)`.
    pub async fn deploy_contract<S: Signer + ?Sized>(
        &self,
        signer: &S,
        bytecode_with_args: Vec<u8>,
        name: &str,
        opts: &TxOptions,
        poll: (u32, Duration),
    ) -> Result<(B256, Address, TxInfo), TronError> {
        let owner = signer.address();
        let (rb, now_ms) = self.tapos().await?;
        let raw = build_create_raw(
            owner,
            bytecode_with_args,
            name,
            0,
            DEFAULT_ORIGIN_ENERGY_LIMIT,
            DEFAULT_USER_FEE_PERCENT,
            rb,
            now_ms,
            opts,
        );
        let signed =
            sign_raw_with(raw, signer).await.map_err(|e| TronError::Decode(e.to_string()))?;
        let local_addr = contract_address_from_txid(signed.txid, owner);
        self.broadcast(&signed).await?;
        let info = self.wait_for_confirmation(signed.txid, poll.0, poll.1).await?;
        match info.contract_address {
            Some(node_addr) if node_addr == local_addr => Ok((signed.txid, local_addr, info)),
            Some(node_addr) => Err(TronError::Decode(format!(
                "deploy address mismatch: local {} != node {}",
                to_hex41(local_addr),
                to_hex41(node_addr),
            ))),
            None => Err(TronError::Decode(format!(
                "node reported no contract_address for deploy {} (local {}); tx may have failed",
                signed.txid,
                to_hex41(local_addr),
            ))),
        }
    }

    /// Calls `contract` from `signer`'s account with the ABI-encoded `data`
    /// (selector + args; ABI encoding is the caller's job) and `call_value` SUN,
    /// then waits for confirmation. Generic over any [`alloy_signer::Signer`].
    pub async fn trigger_contract<S: Signer + ?Sized>(
        &self,
        signer: &S,
        contract: Address,
        call_value: i64,
        data: Vec<u8>,
        opts: &TxOptions,
        poll: (u32, Duration),
    ) -> Result<(B256, TxInfo), TronError> {
        let (rb, now_ms) = self.tapos().await?;
        let raw = build_trigger_raw(signer.address(), contract, call_value, data, rb, now_ms, opts);
        let signed =
            sign_raw_with(raw, signer).await.map_err(|e| TronError::Decode(e.to_string()))?;
        self.broadcast(&signed).await?;
        let info = self.wait_for_confirmation(signed.txid, poll.0, poll.1).await?;
        Ok((signed.txid, info))
    }
}

/// Prefixes a 20-byte address with the 0x41 Tron byte, yielding the 21-byte
/// form protobuf contracts carry.
fn addr21(a: Address) -> Vec<u8> {
    let mut v = Vec::with_capacity(21);
    v.push(0x41);
    v.extend_from_slice(a.as_slice());
    v
}

/// Wraps an already-encoded contract `value` of type `ct` in a `TransactionRaw`
/// with the given TAPOS reference block, timestamp and [`TxOptions`]. The single
/// place `fee_limit` and `expiration` are stamped, so every builder agrees.
fn wrap_raw(
    ct: ContractType,
    value: Vec<u8>,
    rb: RefBlock,
    now_ms: i64,
    opts: &TxOptions,
) -> proto::TransactionRaw {
    proto::TransactionRaw {
        ref_block_bytes: rb.bytes,
        ref_block_hash: rb.hash,
        expiration: now_ms + opts.expiration_ms,
        timestamp: now_ms,
        fee_limit: opts.fee_limit,
        contract: vec![proto::Contract {
            r#type: ct as i32,
            parameter: Some(prost_types::Any { type_url: proto::type_url(ct).to_string(), value }),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Builds the `TransactionRaw` for a native TRX transfer. Deterministic and
/// unit-testable offline: addresses are prefixed with the 0x41 Tron byte and the
/// expiration window comes from `opts`.
pub fn build_transfer_raw(
    owner: Address,
    to: Address,
    amount_sun: i64,
    rb: RefBlock,
    now_ms: i64,
    opts: &TxOptions,
) -> proto::TransactionRaw {
    let transfer = proto::TransferContract {
        owner_address: addr21(owner),
        to_address: addr21(to),
        amount: amount_sun,
    };
    wrap_raw(ContractType::TransferContract, transfer.encode_to_vec(), rb, now_ms, opts)
}

/// Builds the `TransactionRaw` for a `TriggerSmartContract` call. `data` is the
/// ABI-encoded selector + arguments; `call_value` is attached TRX in SUN.
pub fn build_trigger_raw(
    owner: Address,
    contract: Address,
    call_value: i64,
    data: Vec<u8>,
    rb: RefBlock,
    now_ms: i64,
    opts: &TxOptions,
) -> proto::TransactionRaw {
    let trigger = proto::TriggerSmartContract {
        owner_address: addr21(owner),
        contract_address: addr21(contract),
        call_value,
        data,
        call_token_value: 0,
        token_id: 0,
    };
    wrap_raw(ContractType::TriggerSmartContract, trigger.encode_to_vec(), rb, now_ms, opts)
}

/// Builds the `TransactionRaw` for a `CreateSmartContract` deploy.
/// `bytecode_with_args` is the creation bytecode followed by the ABI-encoded
/// constructor arguments; `origin_energy_limit` and `consume_user_resource_percent`
/// are `SmartContract` fields (tags 8 and 6), distinct from the raw's `fee_limit`.
/// The `abi` field is omitted (deployed as empty, matching tronweb `abi: []`).
#[allow(clippy::too_many_arguments)]
pub fn build_create_raw(
    owner: Address,
    bytecode_with_args: Vec<u8>,
    name: &str,
    call_value: i64,
    origin_energy_limit: i64,
    consume_user_resource_percent: i64,
    rb: RefBlock,
    now_ms: i64,
    opts: &TxOptions,
) -> proto::TransactionRaw {
    let owner21 = addr21(owner);
    let create = proto::CreateSmartContract {
        owner_address: owner21.clone(),
        new_contract: Some(proto::SmartContract {
            origin_address: owner21,
            contract_address: Vec::new(),
            bytecode: bytecode_with_args,
            call_value,
            consume_user_resource_percent,
            name: name.to_string(),
            origin_energy_limit,
        }),
        call_token_value: 0,
        token_id: 0,
    };
    wrap_raw(ContractType::CreateSmartContract, create.encode_to_vec(), rb, now_ms, opts)
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
        let raw =
            build_transfer_raw(owner, to, 1_000_000, rb, 1_783_775_034_896, &TxOptions::default());

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

    #[test]
    fn builds_trigger_raw_deterministically() {
        let owner =
            foundry_tron_primitives::address::parse("TX7izXWcmofRYonzdcThrS78jifMtVWCuf").unwrap();
        let contract =
            foundry_tron_primitives::address::parse("TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj").unwrap();
        let data = hex::decode("18160ddd").unwrap();
        let opts = TxOptions { fee_limit: 400_000_000, expiration_ms: 30_000 };
        let rb = RefBlock { bytes: vec![0x3c, 0x6f], hash: vec![1, 2, 3, 4, 5, 6, 7, 8] };
        let raw = build_trigger_raw(owner, contract, 5, data.clone(), rb, 1_783_775_034_896, &opts);

        assert_eq!(raw.expiration, 1_783_775_034_896 + 30_000);
        assert_eq!(raw.fee_limit, 400_000_000);
        assert_eq!(raw.contract[0].r#type, ContractType::TriggerSmartContract as i32);
        let tc = <proto::TriggerSmartContract as Message>::decode(
            raw.contract[0].parameter.as_ref().unwrap().value.as_slice(),
        )
        .unwrap();
        assert_eq!(tc.owner_address[0], 0x41);
        assert_eq!(tc.owner_address[1..], owner.as_slice()[..]);
        assert_eq!(tc.contract_address[1..], contract.as_slice()[..]);
        assert_eq!(tc.call_value, 5);
        assert_eq!(tc.data, data);
    }

    /// Rebuilds the real Nile Counter deploy from its own inputs and asserts the
    /// output is byte-identical to what the node stored, and that the java-tron
    /// deploy-address formula on the rebuilt txid reproduces the node's
    /// `contract_address`. Proves `build_create_raw` emits protobuf java-tron
    /// accepts and mines. The fixture lives in the sibling primitives crate.
    #[test]
    fn build_create_raw_matches_real_nile_deploy() {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../primitives/testdata/nile_create_tx.json"))
                .unwrap();
        let bytes = hex::decode(json["raw_data_hex"].as_str().unwrap()).unwrap();
        let raw = proto::TransactionRaw::decode(bytes.as_slice()).unwrap();
        let owner =
            foundry_tron_primitives::address::parse(json["owner_address"].as_str().unwrap())
                .unwrap();
        let create = <proto::CreateSmartContract as Message>::decode(
            raw.contract[0].parameter.as_ref().unwrap().value.as_slice(),
        )
        .unwrap();
        let sc = create.new_contract.as_ref().unwrap();
        let rb = RefBlock { bytes: raw.ref_block_bytes.clone(), hash: raw.ref_block_hash.clone() };
        let opts =
            TxOptions { fee_limit: raw.fee_limit, expiration_ms: raw.expiration - raw.timestamp };
        let rebuilt = build_create_raw(
            owner,
            sc.bytecode.clone(),
            &sc.name,
            sc.call_value,
            sc.origin_energy_limit,
            sc.consume_user_resource_percent,
            rb,
            raw.timestamp,
            &opts,
        );
        assert_eq!(rebuilt.encode_to_vec(), bytes, "build_create_raw must match the real deploy");

        let txid = proto::txid(&rebuilt);
        assert_eq!(hex::encode(txid), json["txID"].as_str().unwrap());
        assert_eq!(
            to_hex41(contract_address_from_txid(txid, owner)),
            json["contract_address"].as_str().unwrap(),
        );
    }

    #[tokio::test]
    async fn live_deploy_counter_on_nile() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        use std::str::FromStr;
        let key = std::env::var("TRON_PRIVATE_KEY").expect("TRON_PRIVATE_KEY for live deploy");
        let signer = alloy_signer_local::PrivateKeySigner::from_str(&key).unwrap();
        let p = TronProvider::new("https://nile.trongrid.io").unwrap();
        let creation = hex::decode(
            include_str!("../../../evm/core/testdata/tron_counter_creation.hex").trim(),
        )
        .unwrap();
        // 400 TRX cap per the plan's live-test budget.
        let opts = TxOptions { fee_limit: 400_000_000, expiration_ms: 60_000 };
        let poll = (30u32, Duration::from_secs(3));

        let (txid, addr, info) =
            p.deploy_contract(&signer, creation, "Counter", &opts, poll).await.unwrap();
        assert!(info.success, "deploy must succeed");
        // deploy_contract already cross-checks, but assert it here too.
        assert_eq!(info.contract_address, Some(addr));
        eprintln!(
            "live deploy tx {} -> {} ({})",
            hex::encode(txid),
            foundry_tron_primitives::to_base58(addr),
            to_hex41(addr),
        );

        // setNumber(7).
        let mut set = hex::decode("3fb5c1cb").unwrap();
        set.extend_from_slice(&alloy_primitives::U256::from(7u64).to_be_bytes::<32>());
        let (_txid2, info2) = p.trigger_contract(&signer, addr, 0, set, &opts, poll).await.unwrap();
        assert!(info2.success, "setNumber must succeed");

        // number() == 7 via a constant call.
        let cr = p
            .trigger_constant(signer.address(), addr, &hex::decode("8381f58a").unwrap())
            .await
            .unwrap();
        assert!(cr.success);
        assert_eq!(
            alloy_primitives::U256::from_be_slice(&cr.result),
            alloy_primitives::U256::from(7u64),
        );
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
