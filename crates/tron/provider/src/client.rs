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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxInfo {
    pub block_number: i64,
    pub fee_sun: u64,
    /// `receipt.energy_usage_total`: total energy charged, TIP-491 dynamic-energy
    /// penalty already included.
    pub energy_used: u64,
    /// `receipt.energy_penalty_total`: the penalty portion of `energy_used`
    /// (absent on pre-4.7.2 nodes ⇒ 0). Base energy = `energy_used − penalty`.
    pub energy_penalty_total: u64,
    /// `receipt.energy_usage`: energy paid from the caller's own staked energy.
    pub energy_usage_caller: u64,
    /// `receipt.origin_energy_usage`: energy paid by the contract owner's stake
    /// (the `consume_user_resource_percent` share the deployer bankrolls).
    pub origin_energy_usage: u64,
    /// `receipt.net_usage`: bandwidth, in bytes, paid from free/staked bandwidth.
    pub net_usage: u64,
    /// `receipt.net_fee`: bandwidth burned to TRX, in SUN, when free/staked
    /// bandwidth did not cover the transaction.
    pub net_fee_sun: u64,
    pub success: bool,
    pub contract_address: Option<Address>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstantResult {
    pub result: Vec<u8>,
    /// Total metered energy, TIP-491 penalty included (java-tron reports
    /// `energy_used` with the dynamic-energy penalty already added in).
    pub energy_used: u64,
    /// TIP-491 dynamic-energy penalty portion of `energy_used` (4.7.2+ nodes;
    /// absent ⇒ 0). Base energy = `energy_used − energy_penalty`.
    pub energy_penalty: u64,
    pub success: bool,
}

/// Governance chain parameters read from `/wallet/getchainparameters`. These are
/// the live economics — energy price, fee-limit ceiling, bandwidth price, memo
/// fee — and the TIP-491 dynamic-energy knobs the estimate loop (`cast estimate`,
/// fee-limit validation, the gas-report penalty model) depends on. The node
/// returns a `chainParameter` array of `{key, value}` entries; a parameter left
/// at its protobuf default is *omitted* from the array, so a missing key reads as
/// the field's [`Default`] value (which is that live default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TronChainParams {
    /// `getEnergyFee`: SUN burned per energy unit. This is what BASEFEE returns on
    /// Tron and the price used to convert an energy estimate to burned TRX.
    pub energy_fee_sun: u64,
    /// `getMaxFeeLimit`: the ceiling the node accepts for a transaction's
    /// `fee_limit`; a higher `fee_limit` is rejected outright.
    pub max_fee_limit_sun: u64,
    /// `getTransactionFee`: bandwidth price in SUN per byte.
    pub transaction_fee_sun: u64,
    /// `getMemoFee`: SUN charged for a transaction that carries a memo.
    pub memo_fee_sun: u64,
    /// `getDynamicEnergyThreshold`: TIP-491 per-contract energy threshold above
    /// which a maintenance cycle raises the contract's energy factor.
    pub dynamic_threshold: u64,
    /// `getDynamicEnergyIncreaseFactor`: TIP-491 per-cycle increase factor
    /// (precision 10000, i.e. 2000 = +20%).
    pub dynamic_increase_factor: u32,
    /// `getDynamicEnergyMaxFactor`: TIP-491 maximum energy factor (precision
    /// 10000, i.e. 34000 = 3.4, so up to 4.4x total charged energy).
    pub dynamic_max_factor: u32,
    /// `getAllowTvmOsaka`: whether the node runs the Osaka TVM upgrade (0/1). The
    /// local energy/precompile model is pre-Osaka, so a `true` here means the
    /// local simulation may diverge from the node.
    pub allow_tvm_osaka: bool,
}

impl Default for TronChainParams {
    /// Live mainnet values, probed 2026-07-23 via `api.trongrid.io`
    /// `/wallet/getchainparameters`. These are the offline defaults used when the
    /// node cannot be reached (best-effort fetch) and the compile-time baseline
    /// the estimate loop falls back to. Governance can move any of them; the live
    /// staleness sensor (`I5` live-gated test / tron-live CI) prints a diff when
    /// the node no longer matches.
    fn default() -> Self {
        Self {
            energy_fee_sun: 100,
            max_fee_limit_sun: 15_000_000_000,
            transaction_fee_sun: 1_000,
            memo_fee_sun: 1_000_000,
            dynamic_threshold: 5_000_000_000,
            dynamic_increase_factor: 2_000,
            dynamic_max_factor: 34_000,
            allow_tvm_osaka: false,
        }
    }
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

    /// Returns the chain id via the node's partial `eth_*` JSON-RPC endpoint at `<base>/jsonrpc`
    /// (spec §4.5/§4.6: the read-side companion to the `/wallet/*` write API). Tron serves
    /// `eth_chainId` there — not at the wallet base, which rejects JSON-RPC with `405` — so this
    /// points at `/jsonrpc` explicitly and forwards the `TRON-PRO-API-KEY` header when set. Used to
    /// resolve the `broadcast/<script>/<chain>/` directory (Nile = 3448148188).
    pub async fn get_chain_id(&self) -> Result<u64, TronError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "eth_chainId", "params": []
        });
        let mut req = self.client.post(format!("{}/jsonrpc", self.base_url)).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("TRON-PRO-API-KEY", key);
        }
        let v: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
        let hex = v
            .get("result")
            .and_then(|r| r.as_str())
            .ok_or_else(|| TronError::Decode(format!("eth_chainId: no result in {v}")))?;
        let hex = hex.strip_prefix("0x").unwrap_or(hex);
        u64::from_str_radix(hex, 16)
            .map_err(|e| TronError::Decode(format!("eth_chainId: invalid hex {hex}: {e}")))
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

    /// Returns whether `address` is a deployed smart contract on-chain.
    ///
    /// Queries `/wallet/getcontract`: java-tron returns a `SmartContract` object
    /// (carrying `contract_address` and `bytecode`) for a contract account, and an
    /// empty `{}` body for an ordinary account (EOA) or an address it has never
    /// seen — which reads as "not a contract".
    ///
    /// This decides the value-only send path. A value-only send to a contract must
    /// use a `TriggerSmartContract` — an empty-`data` trigger invokes the contract's
    /// payable `receive()`/`fallback()`, matching what `cast send <contract> --value`
    /// does on Ethereum. A bare `TransferContract` would only credit the balance
    /// without running the contract's code, and java-tron additionally rejects it
    /// outright when governance activates it (`getForbidTransferToContract == 1`, or
    /// `getAllowTvmCompatibleEvm == 1` for a version-1 contract). An ordinary account
    /// takes the native `TransferContract`. Transient errors propagate so the caller
    /// decides whether to fall back.
    pub async fn is_contract(&self, address: Address) -> Result<bool, TronError> {
        let body = serde_json::json!({ "value": to_hex41(address), "visible": false });
        let v: serde_json::Value = self.post_json("/wallet/getcontract", body).await?;
        Ok(parse_is_contract(&v))
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

    /// Fetches the node's governance chain parameters (`/wallet/getchainparameters`).
    /// Any parameter the node left at its protobuf default is omitted from the
    /// response and reads as the corresponding [`TronChainParams`] default, so the
    /// returned struct always carries a usable value for every field.
    pub async fn get_chain_parameters(&self) -> Result<TronChainParams, TronError> {
        let v: serde_json::Value =
            self.post_json("/wallet/getchainparameters", serde_json::json!({})).await?;
        Ok(parse_chain_parameters(&v))
    }

    /// Estimates the energy a `TriggerSmartContract` call from `owner` to
    /// `contract` with the ABI-encoded `data` would consume, via
    /// `/wallet/estimateenergy` (4.7.0.1+, gated on the node's `vm.estimateEnergy`
    /// + `vm.supportConstant`).
    ///
    /// Returns `Some(energy_required)` on a supporting node. Returns `Ok(None)`
    /// when the node has the endpoint disabled (the documented
    /// "this node does not support estimate energy" validate error) — the signal
    /// for the caller to fall back to [`trigger_constant`], whose `energy_used`
    /// already includes the TIP-491 penalty. Any other node/decode error (a
    /// revert, bad arguments) propagates.
    pub async fn estimate_energy(
        &self,
        owner: Address,
        contract: Address,
        data: &[u8],
    ) -> Result<Option<u64>, TronError> {
        let body = serde_json::json!({
            "owner_address": to_hex41(owner),
            "contract_address": to_hex41(contract),
            "data": hex::encode(data),
            "visible": false,
        });
        let v: serde_json::Value = self.post_json("/wallet/estimateenergy", body).await?;
        parse_estimate_energy(&v)
    }

    /// Returns `contract`'s current TIP-491 dynamic-energy factor (precision
    /// 10000, i.e. 34000 = 3.4x, up to 4.4x total charged energy) from
    /// `/wallet/getcontractinfo`'s `contract_state.energy_factor`. A fresh or cold
    /// contract has accrued no factor and the field is absent ⇒ 0.
    pub async fn get_contract_energy_factor(&self, contract: Address) -> Result<u32, TronError> {
        let body = serde_json::json!({ "value": to_hex41(contract), "visible": false });
        let v: serde_json::Value = self.post_json("/wallet/getcontractinfo", body).await?;
        Ok(parse_energy_factor(&v))
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

/// `Constant.MAX_RESULT_SIZE_IN_TX` (java-tron `common/.../Constant.java`): java-tron charges an
/// extra 64 bandwidth bytes per contract for the transaction's reserved result slot.
const MAX_RESULT_SIZE_IN_TX: u64 = 64;

/// Placeholder wall-clock (ms since epoch) used only for offline bandwidth estimation. Any epoch in
/// `[2^35, 2^42)` ms serializes `expiration` and `timestamp` as fixed 6-byte varints, so the exact
/// value never affects the byte count; this one (~2023-11) stays inside that window for decades.
const ESTIMATE_NOW_MS: i64 = 1_700_000_000_000;

/// Placeholder TAPOS reference block for offline estimation. `ref_block_bytes` is always 2 bytes
/// and `ref_block_hash` always 8 bytes on the wire regardless of content, so zeroed fields
/// reproduce the exact serialized size of a real reference block.
fn estimate_ref_block() -> RefBlock {
    RefBlock { bytes: vec![0u8; 2], hash: vec![0u8; 8] }
}

/// Bandwidth, in bytes, that java-tron charges to broadcast the signed transaction carrying `raw`:
/// `serialized_size(signed tx, ret cleared) + MAX_RESULT_SIZE_IN_TX`
/// (`BandwidthProcessor.consume`). A freshly built tx has an empty `ret`, so `clearRet()` is a
/// no-op; the signature is a fixed 65-byte dummy (`PER_SIGN_LENGTH`) whose content does not change
/// the size.
fn signed_bandwidth(raw: proto::TransactionRaw) -> u64 {
    let tx = proto::Transaction { raw_data: Some(raw), signature: vec![vec![0u8; 65]] };
    tx.encode_to_vec().len() as u64 + MAX_RESULT_SIZE_IN_TX
}

/// Estimates the on-chain bandwidth, in bytes, that java-tron charges to broadcast a
/// `TriggerSmartContract` call carrying `data` (ABI-encoded selector + args) with `call_value` SUN
/// attached, under the fee limit in `opts`.
///
/// Exact for transactions built by this crate's broadcast path (same builder); a third-party wallet
/// that additionally sets `ref_block_num` would serialize a few bytes larger.
pub fn estimate_call_bandwidth(data: Vec<u8>, call_value: i64, opts: &TxOptions) -> u64 {
    let raw = build_trigger_raw(
        Address::ZERO,
        Address::ZERO,
        call_value,
        data,
        estimate_ref_block(),
        ESTIMATE_NOW_MS,
        opts,
    );
    signed_bandwidth(raw)
}

/// Estimates the on-chain bandwidth, in bytes, that java-tron charges to broadcast a
/// `CreateSmartContract` deploy of `bytecode_with_args` (creation bytecode followed by ABI-encoded
/// constructor args) named `name`, with the given `SmartContract` energy fields and `opts`.
///
/// Exact for transactions built by this crate's broadcast path; see [`estimate_call_bandwidth`].
#[allow(clippy::too_many_arguments)]
pub fn estimate_create_bandwidth(
    bytecode_with_args: Vec<u8>,
    name: &str,
    call_value: i64,
    origin_energy_limit: i64,
    consume_user_resource_percent: i64,
    opts: &TxOptions,
) -> u64 {
    let raw = build_create_raw(
        Address::ZERO,
        bytecode_with_args,
        name,
        call_value,
        origin_energy_limit,
        consume_user_resource_percent,
        estimate_ref_block(),
        ESTIMATE_NOW_MS,
        opts,
    );
    signed_bandwidth(raw)
}

/// Default headroom, in percent, added to the raw energy cost when suggesting a
/// `fee_limit`. Twenty percent absorbs the TIP-491 dynamic-energy factor moving up
/// by up to one maintenance cycle (+20%) between the estimate and the broadcast.
pub const DEFAULT_FEE_LIMIT_BUFFER_PCT: u64 = 20;

/// Suggests a `fee_limit` in SUN for a transaction that consumes `energy_total`
/// energy: the energy cost at the node's live price (`params.energy_fee_sun`)
/// inflated by `buffer_pct` percent of headroom, then clamped to the node's
/// `getMaxFeeLimit`. The clamp guarantees the node accepts the limit — a
/// `fee_limit` above `getMaxFeeLimit` is rejected outright. Intermediate
/// arithmetic is `u128` so a large energy estimate cannot overflow before the
/// clamp.
pub fn suggest_fee_limit_sun(energy_total: u64, params: &TronChainParams, buffer_pct: u64) -> u64 {
    let raw = (energy_total as u128)
        .saturating_mul(params.energy_fee_sun as u128)
        .saturating_mul(100 + buffer_pct as u128)
        / 100;
    raw.min(params.max_fee_limit_sun as u128) as u64
}

/// Validates a `fee_limit` (SUN) against the node's `getMaxFeeLimit` ceiling and
/// the positivity the protocol requires. Returns a human-readable reason when the
/// limit is unusable so the CLI caller can surface it (a `fee_limit` above
/// `getMaxFeeLimit`, or non-positive, is rejected by the node), `Ok(())`
/// otherwise.
pub fn check_fee_limit(fee_limit: i64, params: &TronChainParams) -> Result<(), String> {
    if fee_limit <= 0 {
        return Err(format!("tron fee_limit must be positive (got {fee_limit} SUN)"));
    }
    if fee_limit as u128 > params.max_fee_limit_sun as u128 {
        return Err(format!(
            "tron fee_limit {fee_limit} SUN exceeds the node's getMaxFeeLimit {} SUN ({} TRX); \
             lower it with --tron.fee-limit or the [tron] fee_limit config",
            params.max_fee_limit_sun,
            params.max_fee_limit_sun / 1_000_000,
        ));
    }
    Ok(())
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

/// Parses a `/wallet/getcontract` response into "is this address a contract?".
/// java-tron returns a `SmartContract` object carrying `contract_address` for a
/// deployed contract and an empty `{}` body for an ordinary account, so the
/// presence of a `contract_address` string is the discriminator.
pub(crate) fn parse_is_contract(v: &serde_json::Value) -> bool {
    v.get("contract_address").and_then(|c| c.as_str()).is_some()
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
    let ru64 = |field: &str| receipt.get(field).and_then(|e| e.as_u64()).unwrap_or(0);
    Ok(Some(TxInfo {
        block_number,
        fee_sun: v.get("fee").and_then(|f| f.as_u64()).unwrap_or(0),
        energy_used: ru64("energy_usage_total"),
        energy_penalty_total: ru64("energy_penalty_total"),
        energy_usage_caller: ru64("energy_usage"),
        origin_energy_usage: ru64("origin_energy_usage"),
        net_usage: ru64("net_usage"),
        net_fee_sun: ru64("net_fee"),
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
        energy_penalty: v.get("energy_penalty").and_then(|e| e.as_u64()).unwrap_or(0),
        success,
    })
}

/// Parses a `/wallet/getchainparameters` response into [`TronChainParams`],
/// starting from the live defaults and overriding each field whose `key` is
/// present in the `chainParameter` array. A key the node omits (its value equals
/// the protobuf default) leaves the field at its default, which is that same live
/// value.
pub(crate) fn parse_chain_parameters(v: &serde_json::Value) -> TronChainParams {
    let mut params = TronChainParams::default();
    let Some(entries) = v.get("chainParameter").and_then(|c| c.as_array()) else {
        return params;
    };
    let get = |key: &str| -> Option<i64> {
        entries
            .iter()
            .find(|e| e.get("key").and_then(|k| k.as_str()) == Some(key))
            .and_then(|e| e.get("value"))
            .and_then(|val| val.as_i64())
    };
    if let Some(x) = get("getEnergyFee") {
        params.energy_fee_sun = x as u64;
    }
    if let Some(x) = get("getMaxFeeLimit") {
        params.max_fee_limit_sun = x as u64;
    }
    if let Some(x) = get("getTransactionFee") {
        params.transaction_fee_sun = x as u64;
    }
    if let Some(x) = get("getMemoFee") {
        params.memo_fee_sun = x as u64;
    }
    if let Some(x) = get("getDynamicEnergyThreshold") {
        params.dynamic_threshold = x as u64;
    }
    if let Some(x) = get("getDynamicEnergyIncreaseFactor") {
        params.dynamic_increase_factor = x as u32;
    }
    if let Some(x) = get("getDynamicEnergyMaxFactor") {
        params.dynamic_max_factor = x as u32;
    }
    // Osaka is omitted (protobuf default 0) until governance activates it; a
    // present, non-zero value flips it on.
    params.allow_tvm_osaka = get("getAllowTvmOsaka").unwrap_or(0) != 0;
    params
}

/// Parses a `/wallet/estimateenergy` response. `Some(energy_required)` on a
/// supporting node, `Ok(None)` when the node reports it does not support the
/// endpoint (the fall-back signal), and `Err` for any other error.
pub(crate) fn parse_estimate_energy(v: &serde_json::Value) -> Result<Option<u64>, TronError> {
    // Supporting node: { "result": { "result": true }, "energy_required": N }.
    if let Some(required) = v.get("energy_required").and_then(|e| e.as_u64()) {
        return Ok(Some(required));
    }
    // Disabled endpoint: a CONTRACT_VALIDATE_ERROR whose (hex) message decodes to
    // "... does not support estimate energy". That is the fall-back signal, not a
    // hard error.
    let code = v["result"].get("code").and_then(|c| c.as_str()).unwrap_or_default();
    let raw_msg = v["result"].get("message").and_then(|m| m.as_str()).unwrap_or_default();
    let message = hex::decode(raw_msg)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| raw_msg.to_string());
    if message.contains("does not support estimate energy") {
        return Ok(None);
    }
    // Any other failure (a revert, bad arguments, a different disablement) propagates.
    Err(TronError::Api { code: code.to_string(), message })
}

/// Parses `contract_state.energy_factor` (TIP-491 dynamic-energy factor, precision
/// 10000) from a `/wallet/getcontractinfo` response. Absent (fresh/cold contract,
/// or non-contract `{}` body) ⇒ 0.
pub(crate) fn parse_energy_factor(v: &serde_json::Value) -> u32 {
    v.get("contract_state")
        .and_then(|s| s.get("energy_factor"))
        .and_then(|f| f.as_u64())
        .unwrap_or(0) as u32
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
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_primitives::{Bytes, TxKind, U256};
    use foundry_evm_core::evm::TronEvmFactory;
    use revm::{
        context::{CfgEnv, TxEnv},
        database::{CacheDB, EmptyDB},
        primitives::hardfork::SpecId,
        state::{AccountInfo, Bytecode},
    };

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

    #[tokio::test]
    async fn live_get_chain_id_nile() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        let p = TronProvider::new("https://nile.trongrid.io").unwrap();
        // Nile chain id, verified against `eth_chainId` (0xcd8690dc).
        assert_eq!(p.get_chain_id().await.unwrap(), 3_448_148_188);
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

    /// A `/wallet/getcontract` reply for a deployed contract classifies as a
    /// contract. Fixture: the real mainnet response for USDT
    /// (`TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t`), captured 2026-07-23, with the 31KB
    /// `bytecode` kept as its real leading prefix and the large `abi.entrys` array
    /// dropped for size — `contract_address`, on which the decision keys, is
    /// verbatim.
    #[test]
    fn getcontract_usdt_is_contract() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/mainnet_getcontract_usdt.json"))
                .unwrap();
        assert!(parse_is_contract(&v));
        // The markers the discriminator relies on are present in a real contract reply.
        assert!(v.get("contract_address").and_then(|c| c.as_str()).is_some());
        assert!(v.get("bytecode").and_then(|b| b.as_str()).is_some_and(|b| !b.is_empty()));
    }

    /// A `/wallet/getcontract` reply for an ordinary account is an empty `{}`
    /// body, which classifies as not-a-contract. Fixture: the real mainnet empty
    /// reply for the EOA `TWd4WrZ9wn84f5x1hZhL4DHvk738ns5jwb`
    /// (`41e28b3cfd4e0e909077821478e9fcb86b84be786e`), captured 2026-07-23.
    #[test]
    fn getcontract_eoa_is_not_contract() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/mainnet_getcontract_eoa.json")).unwrap();
        assert!(!parse_is_contract(&v));
    }

    #[test]
    fn getcontract_empty_object_is_not_contract() {
        assert!(!parse_is_contract(&serde_json::json!({})));
    }

    /// The node distinguishes a contract from an EOA via `/wallet/getcontract`.
    /// Mainnet is read-only here (no transaction, no TRX): USDT is a contract, a
    /// large USDT holder (`TWd4WrZ9wn84f5x1hZhL4DHvk738ns5jwb`) is an EOA.
    #[tokio::test]
    async fn live_is_contract_distinguishes_usdt_and_eoa_on_mainnet() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live mainnet read-only tests");
            return;
        }
        let p = TronProvider::new("https://api.trongrid.io").unwrap();
        let usdt =
            foundry_tron_primitives::address::parse("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").unwrap();
        let eoa =
            foundry_tron_primitives::address::parse("TWd4WrZ9wn84f5x1hZhL4DHvk738ns5jwb").unwrap();
        assert!(p.is_contract(usdt).await.unwrap(), "USDT must be a contract");
        assert!(!p.is_contract(eoa).await.unwrap(), "a plain holder must be an EOA");
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

    /// The chain-parameters parser maps the real mainnet `getchainparameters`
    /// response onto every `TronChainParams` field. Fixture: the verbatim mainnet
    /// reply captured 2026-07-23 (all ~75 governance keys), so the parser is
    /// proven to find its keys among the full set the node returns.
    #[test]
    fn parses_chain_parameters_mainnet_fixture() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/mainnet_getchainparameters.json"))
                .unwrap();
        let p = parse_chain_parameters(&v);
        assert_eq!(p.energy_fee_sun, 100, "getEnergyFee");
        assert_eq!(p.max_fee_limit_sun, 15_000_000_000, "getMaxFeeLimit");
        assert_eq!(p.transaction_fee_sun, 1_000, "getTransactionFee (bandwidth sun/byte)");
        assert_eq!(p.memo_fee_sun, 1_000_000, "getMemoFee");
        assert_eq!(p.dynamic_threshold, 5_000_000_000, "getDynamicEnergyThreshold");
        assert_eq!(p.dynamic_increase_factor, 2_000, "getDynamicEnergyIncreaseFactor");
        assert_eq!(p.dynamic_max_factor, 34_000, "getDynamicEnergyMaxFactor");
        // getAllowTvmOsaka is absent from the mainnet response (protobuf default),
        // so it reads as pre-Osaka.
        assert!(!p.allow_tvm_osaka, "getAllowTvmOsaka is absent => pre-Osaka");
    }

    /// A missing key keeps the field at its live default; the whole parser falls
    /// back to defaults when the `chainParameter` array is absent.
    #[test]
    fn chain_parameters_default_on_missing_keys() {
        // Empty body => every field is the offline default.
        assert_eq!(parse_chain_parameters(&serde_json::json!({})), TronChainParams::default());
        // Only getEnergyFee present (governance moved it to 210) and Osaka on: the
        // named keys change, all others stay at the default.
        let v = serde_json::json!({
            "chainParameter": [
                { "key": "getEnergyFee", "value": 210 },
                { "key": "getAllowTvmOsaka", "value": 1 },
            ]
        });
        let p = parse_chain_parameters(&v);
        assert_eq!(p.energy_fee_sun, 210, "present key overrides the default");
        assert!(p.allow_tvm_osaka, "present, non-zero Osaka flips it on");
        assert_eq!(p.max_fee_limit_sun, 15_000_000_000, "absent key keeps the default");
        assert_eq!(p.dynamic_max_factor, 34_000, "absent key keeps the default");
    }

    /// Live (mainnet, read-only) staleness sensor for the chain-parameter
    /// constants: the fetched values are compared against the plan's 2026-07
    /// snapshot (`TronChainParams::default()`) and the diff is PRINTED, not
    /// asserted equal, so the tron-live CI surfaces a governance drift without a
    /// hard failure. `getEnergyFee` is additionally asserted `> 0` (a sanity floor
    /// the estimate loop relies on).
    #[tokio::test]
    async fn live_chain_parameters_match_snapshot_on_mainnet() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live mainnet read-only tests");
            return;
        }
        let p = TronProvider::new("https://api.trongrid.io").unwrap();
        let live = p.get_chain_parameters().await.unwrap();
        let snap = TronChainParams::default();
        if live != snap {
            eprintln!(
                "chain-parameter drift vs 2026-07 snapshot:\n live: {live:?}\n snap: {snap:?}"
            );
        }
        assert!(live.energy_fee_sun > 0, "getEnergyFee must be positive");
        assert!(live.max_fee_limit_sun > 0, "getMaxFeeLimit must be positive");
    }

    /// The constant-call parser reads the TIP-491 `energy_penalty` alongside
    /// `energy_used`. Fixture: the real mainnet `triggerconstantcontract` reply for
    /// USDT `transfer(address,uint256)` (captured 2026-07-23), whose
    /// `energy_used = 64285` already includes `energy_penalty = 49635`, so the base
    /// energy the local model reproduces is `64285 − 49635 = 14650`.
    #[test]
    fn parses_constant_result_energy_penalty_mainnet_usdt() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/mainnet_triggerconstant_usdt.json"))
                .unwrap();
        let cr = parse_constant_result(&v).unwrap();
        assert!(cr.success);
        assert_eq!(cr.energy_used, 64285, "energy_used includes the penalty");
        assert_eq!(cr.energy_penalty, 49635, "TIP-491 penalty portion");
        assert_eq!(cr.energy_used - cr.energy_penalty, 14650, "base = used − penalty");
    }

    /// A pre-4.7.2 reply without `energy_penalty` reads the penalty as 0 (the
    /// existing Nile totalSupply fixture carries no penalty field).
    #[test]
    fn constant_result_penalty_defaults_to_zero() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/nile_triggerconstant.json")).unwrap();
        assert_eq!(parse_constant_result(&v).unwrap().energy_penalty, 0);
    }

    /// The tx-info parser reads the extended receipt resource fields. Fixture: the
    /// real mainnet receipt for a USDT `transfer` (captured 2026-07-23) whose
    /// caller paid energy from stake and burned bandwidth to TRX.
    #[test]
    fn parses_tx_info_resource_fields_mainnet_usdt() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/mainnet_txinfo_usdt.json")).unwrap();
        let info = parse_tx_info(&v).unwrap().expect("mined tx is in a block");
        assert!(info.success);
        assert_eq!(info.block_number, 84_706_816);
        assert_eq!(info.energy_used, 64285, "energy_usage_total");
        assert_eq!(info.energy_penalty_total, 49635, "energy_penalty_total");
        assert_eq!(info.energy_usage_caller, 64285, "receipt.energy_usage (caller stake)");
        assert_eq!(info.net_fee_sun, 345000, "receipt.net_fee (bandwidth burned to SUN)");
        // Not present in this receipt ⇒ default 0.
        assert_eq!(info.origin_energy_usage, 0);
        assert_eq!(info.net_usage, 0);
        assert_eq!(info.energy_used - info.energy_penalty_total, 14650, "base = used − penalty");
    }

    /// A supporting node's `estimateenergy` reply yields `Some(energy_required)`.
    /// The response shape follows java-tron's `EstimateEnergyMessage`
    /// (`result` + `energy_required`); public TronGrid/Nile disable the endpoint,
    /// so the supported shape is exercised against the documented schema while the
    /// disabled shape below is a captured live fixture.
    #[test]
    fn estimate_energy_supported_returns_some() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/estimateenergy_supported.json"))
                .unwrap();
        assert_eq!(parse_estimate_energy(&v).unwrap(), Some(64285));
    }

    /// A node with the endpoint disabled returns a CONTRACT_VALIDATE_ERROR whose
    /// message decodes to "this node does not support estimate energy"; the parser
    /// maps it to `Ok(None)` (the fall-back signal). Fixture: the real mainnet
    /// reply captured 2026-07-23.
    #[test]
    fn estimate_energy_unsupported_returns_none() {
        let v: serde_json::Value = serde_json::from_str(include_str!(
            "../testdata/mainnet_estimateenergy_unsupported.json"
        ))
        .unwrap();
        assert_eq!(parse_estimate_energy(&v).unwrap(), None);
    }

    /// Any other estimateenergy failure (e.g. a revert) propagates as an error,
    /// never silently as `None`.
    #[test]
    fn estimate_energy_other_error_propagates() {
        let v = serde_json::json!({
            "result": { "code": "CONTRACT_EXE_ERROR", "message": hex::encode("REVERT opcode executed") }
        });
        let err = parse_estimate_energy(&v).unwrap_err();
        assert!(matches!(err, TronError::Api { .. }));
    }

    /// The energy-factor parser reads `contract_state.energy_factor`. Fixtures: the
    /// real mainnet `getcontractinfo` for USDT (a hot contract pinned at the max
    /// factor 34000) and for a cold contract whose `contract_state` omits the
    /// field ⇒ 0; an empty `{}` (non-contract) is also 0.
    #[test]
    fn parses_energy_factor_mainnet_fixtures() {
        let hot: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/mainnet_getcontractinfo_usdt.json"))
                .unwrap();
        assert_eq!(parse_energy_factor(&hot), 34000, "USDT is at the max energy factor");
        let cold: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/mainnet_getcontractinfo_cold.json"))
                .unwrap();
        assert_eq!(parse_energy_factor(&cold), 0, "cold contract has no accrued factor");
        assert_eq!(parse_energy_factor(&serde_json::json!({})), 0, "non-contract ⇒ 0");
    }

    /// Live (mainnet, read-only) USDT estimate loop: the constant call reports a
    /// non-zero TIP-491 penalty, `getcontractinfo` reports a positive energy
    /// factor, and `estimateenergy` on the public node signals it is unsupported
    /// (⇒ the `trigger_constant` fallback). Spends no TRX.
    #[tokio::test]
    async fn live_usdt_estimate_loop_on_mainnet() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live mainnet read-only tests");
            return;
        }
        let p = TronProvider::new("https://api.trongrid.io").unwrap();
        let usdt =
            foundry_tron_primitives::address::parse("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").unwrap();
        // A large USDT holder (real, funded), used only as the constant-call owner.
        let holder =
            foundry_tron_primitives::address::parse("TWd4WrZ9wn84f5x1hZhL4DHvk738ns5jwb").unwrap();
        // transfer(holder, 1): selector a9059cbb + address word + amount 1.
        let mut data = hex::decode("a9059cbb").unwrap();
        data.extend_from_slice(&{
            let mut w = [0u8; 32];
            w[12..].copy_from_slice(holder.as_slice());
            w
        });
        data.extend_from_slice(&alloy_primitives::U256::from(1u64).to_be_bytes::<32>());

        let cr = p.trigger_constant(holder, usdt, &data).await.unwrap();
        assert!(cr.success, "constant transfer must succeed");
        assert!(cr.energy_penalty > 0, "USDT carries a TIP-491 penalty");
        assert!(cr.energy_used > cr.energy_penalty, "base = used − penalty must be positive");

        let factor = p.get_contract_energy_factor(usdt).await.unwrap();
        assert!(factor > 0, "USDT has a non-zero dynamic-energy factor");

        // Public TronGrid disables estimateenergy ⇒ None (fall-back signal).
        assert_eq!(
            p.estimate_energy(holder, usdt, &data).await.unwrap(),
            None,
            "public node signals estimateenergy is unsupported",
        );
    }

    #[test]
    fn suggest_fee_limit_applies_buffer_and_clamp() {
        let params = TronChainParams::default(); // energy 100 sun, max 15_000_000_000.
        // A USDT-class transfer (~64285 energy) at 100 sun with the default 20%
        // buffer: 64285 * 100 * 1.2 = 7_714_200 SUN (~7.7 TRX), well under the cap.
        assert_eq!(suggest_fee_limit_sun(64_285, &params, DEFAULT_FEE_LIMIT_BUFFER_PCT), 7_714_200,);
        // Zero buffer is the bare energy cost.
        assert_eq!(suggest_fee_limit_sun(64_285, &params, 0), 6_428_500);
        // A huge estimate clamps to getMaxFeeLimit (15_000 TRX): 200e6 * 100 * 1.2
        // = 24e9 > 15e9 ⇒ clamped.
        assert_eq!(
            suggest_fee_limit_sun(200_000_000, &params, DEFAULT_FEE_LIMIT_BUFFER_PCT),
            15_000_000_000,
        );
        // Zero energy ⇒ zero suggestion.
        assert_eq!(suggest_fee_limit_sun(0, &params, DEFAULT_FEE_LIMIT_BUFFER_PCT), 0);
    }

    #[test]
    fn check_fee_limit_rejects_over_max_and_non_positive() {
        let params = TronChainParams::default();
        assert!(check_fee_limit(1_000_000_000, &params).is_ok(), "1000 TRX is under the cap");
        assert!(check_fee_limit(15_000_000_000, &params).is_ok(), "exactly getMaxFeeLimit is ok");
        assert!(
            check_fee_limit(15_000_000_001, &params).is_err(),
            "one SUN over getMaxFeeLimit is rejected"
        );
        assert!(check_fee_limit(0, &params).is_err(), "zero fee_limit is rejected");
        assert!(check_fee_limit(-1, &params).is_err(), "negative fee_limit is rejected");
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

    /// `build_trigger_raw` accepts empty `data`: a value-only contract call
    /// (payable fallback/receive) is a legal `TriggerSmartContract`. The builder
    /// must not reject the empty vector — it is exactly what the transfer-bug fix
    /// routes a value-only call to a contract through.
    #[test]
    fn builds_trigger_raw_with_empty_data_is_value_only_call() {
        let owner =
            foundry_tron_primitives::address::parse("TX7izXWcmofRYonzdcThrS78jifMtVWCuf").unwrap();
        let contract =
            foundry_tron_primitives::address::parse("TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj").unwrap();
        let rb = RefBlock { bytes: vec![0x3c, 0x6f], hash: vec![1, 2, 3, 4, 5, 6, 7, 8] };
        let raw = build_trigger_raw(
            owner,
            contract,
            1_000_000,
            Vec::new(),
            rb,
            1_783_775_034_896,
            &TxOptions::default(),
        );
        assert_eq!(raw.contract[0].r#type, ContractType::TriggerSmartContract as i32);
        let tc = <proto::TriggerSmartContract as Message>::decode(
            raw.contract[0].parameter.as_ref().unwrap().value.as_slice(),
        )
        .unwrap();
        assert_eq!(tc.contract_address[1..], contract.as_slice()[..]);
        assert_eq!(tc.call_value, 1_000_000);
        assert!(tc.data.is_empty(), "value-only call carries empty calldata");
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

    /// Signed-transaction size of a fixture's own `raw_data`, plus the +64 result slot: the
    /// ground-truth bandwidth against which the estimators are checked. Reuses the fixture's
    /// byte-identical `raw_data` (proven to round-trip in the primitives crate).
    fn fixture_bandwidth(raw: proto::TransactionRaw) -> u64 {
        signed_bandwidth(raw)
    }

    /// The call-bandwidth estimator reproduces the real mainnet `transfer(address,uint256)`
    /// fixture's on-chain bandwidth (345 bytes = node `net_fee` 345000 SUN / 1000 SUN-per-byte),
    /// both directly from the fixture's `raw_data` and by rebuilding from its semantic inputs.
    #[test]
    fn estimate_call_bandwidth_matches_mainnet_trigger_fixture() {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../primitives/testdata/mainnet_trigger_tx.json"))
                .unwrap();
        let bytes = hex::decode(json["raw_data_hex"].as_str().unwrap()).unwrap();
        let raw = proto::TransactionRaw::decode(bytes.as_slice()).unwrap();

        // Ground truth: the fixture's own raw_data wrapped in a signed tx is 345 bandwidth bytes.
        assert_eq!(fixture_bandwidth(raw.clone()), 345);

        // Estimator: rebuilding from the semantic inputs (calldata, call_value, fee_limit)
        // reproduces the exact byte count. Placeholder TAPOS/timestamp match the fixture's widths.
        let trigger = <proto::TriggerSmartContract as Message>::decode(
            raw.contract[0].parameter.as_ref().unwrap().value.as_slice(),
        )
        .unwrap();
        let opts =
            TxOptions { fee_limit: raw.fee_limit, expiration_ms: raw.expiration - raw.timestamp };
        assert_eq!(estimate_call_bandwidth(trigger.data, trigger.call_value, &opts), 345);
    }

    /// The create-bandwidth estimator reproduces the real Nile Counter deploy fixture's on-chain
    /// bandwidth (853 bytes), both directly from the fixture and by rebuilding from its inputs.
    #[test]
    fn estimate_create_bandwidth_matches_nile_deploy_fixture() {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../primitives/testdata/nile_create_tx.json"))
                .unwrap();
        let bytes = hex::decode(json["raw_data_hex"].as_str().unwrap()).unwrap();
        let raw = proto::TransactionRaw::decode(bytes.as_slice()).unwrap();

        // Ground truth: the fixture's own raw_data wrapped in a signed tx is 853 bandwidth bytes.
        assert_eq!(fixture_bandwidth(raw.clone()), 853);

        // Estimator: rebuilding from the deploy's semantic inputs reproduces the exact byte count.
        let create = <proto::CreateSmartContract as Message>::decode(
            raw.contract[0].parameter.as_ref().unwrap().value.as_slice(),
        )
        .unwrap();
        let sc = create.new_contract.as_ref().unwrap();
        let opts =
            TxOptions { fee_limit: raw.fee_limit, expiration_ms: raw.expiration - raw.timestamp };
        assert_eq!(
            estimate_create_bandwidth(
                sc.bytecode.clone(),
                &sc.name,
                sc.call_value,
                sc.origin_energy_limit,
                sc.consume_user_resource_percent,
                &opts,
            ),
            853,
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

    /// Live (Nile) proof of the value-only-to-contract fix (plan I2 gate). Deploys
    /// a minimal payable sink (runtime `0x00` / STOP — a contract that accepts a
    /// value-bearing call with empty calldata and keeps the TRX), then:
    ///   1. the node classifies the sink as a contract and the funded EOA as not (the `is_contract`
    ///      discriminator that drives the send-path choice);
    ///   2. the fix — an empty-`data` `TriggerSmartContract` carrying `call_value` — is ACCEPTED
    ///      (receipt SUCCESS) and the sink's balance grows by the value, which is exactly what
    ///      `cast send`/`forge script` now build for a value-only send to a contract.
    ///
    /// The pre-fix path built a `TransferContract`, whose correctness bug is that it only credits
    /// the contract's balance and never runs its `receive()`/`fallback()`, so `cast send <contract>
    /// --value` would silently skip the payable entry point the user means to invoke (`cast send`'s
    /// Ethereum semantics run it). java-tron *additionally* rejects a bare transfer to a contract,
    /// but only under governance: `TransferActuator.validate()` throws iff
    /// `getForbidTransferToContract == 1`, or `getAllowTvmCompatibleEvm == 1` and the target's
    /// contract version is 1. Both flags are 0 on mainnet and Nile as of 2026-07 (their `value`
    /// is omitted from `getchainparameters`, the protobuf default), so a live bare transfer is
    /// currently accepted — which is why this gate proves the fix through the always-true
    /// semantic path (the trigger runs contract code) rather than asserting a node rejection
    /// that governance can toggle.
    #[tokio::test]
    async fn live_value_only_contract_call_on_nile() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        use std::str::FromStr;
        let key = std::env::var("TRON_PRIVATE_KEY").expect("TRON_PRIVATE_KEY for live value-only");
        let signer = alloy_signer_local::PrivateKeySigner::from_str(&key).unwrap();
        // nileex.io: nile.trongrid.io is unreachable from this host.
        let p = TronProvider::new("https://api.nileex.io").unwrap();
        let owner = signer.address();

        // Minimal payable sink: creation returns a single-byte STOP runtime, so a value-bearing
        // call with empty calldata halts successfully and the TRX stays with the contract.
        let creation = hex::decode("6001600c60003960016000f300").unwrap();
        let opts = TxOptions { fee_limit: 400_000_000, expiration_ms: 60_000 };
        let poll = (30u32, Duration::from_secs(3));
        let (deploy_txid, sink, info) =
            p.deploy_contract(&signer, creation, "PayableSink", &opts, poll).await.unwrap();
        assert!(info.success, "sink deploy must succeed");
        eprintln!(
            "live payable sink tx {} -> {}",
            hex::encode(deploy_txid),
            foundry_tron_primitives::to_base58(sink),
        );

        // 1) The node distinguishes the contract from the funded EOA: this is the discriminator the
        //    fix routes on.
        assert!(p.is_contract(sink).await.unwrap(), "sink must classify as a contract");
        assert!(!p.is_contract(owner).await.unwrap(), "the funded signer is an EOA");

        // 2) The fix: an empty-data trigger with call_value runs the contract and credits the sink.
        let value_sun = 1_000_000i64;
        let before = p.get_balance(sink).await.unwrap();
        let (call_txid, call_info) =
            p.trigger_contract(&signer, sink, value_sun, Vec::new(), &opts, poll).await.unwrap();
        assert!(call_info.success, "empty-data trigger with value must succeed");
        let after = p.get_balance(sink).await.unwrap();
        assert_eq!(
            after - before,
            value_sun as u64,
            "the sink balance must grow by the transferred value",
        );
        eprintln!(
            "live value-only fix: trigger tx https://nile.tronscan.org/#/transaction/{} credited {value_sun} SUN",
            hex::encode(call_txid),
        );
    }

    /// Live confirmation of the two highest-risk Tron precompile divergences on
    /// Nile: `0x01` ECRecover's 21-byte (`0x41`-prefixed) address word and
    /// `0x03`'s `sha256(sha256(x)[..20])` (not ripemd160). Precompile addresses
    /// are not directly callable via `triggerconstantcontract` ("Smart contract
    /// is not exist"), so this deploys a tiny generic STATICCALL proxy whose
    /// calldata is `target_word(32) ‖ input`, then probes it. Cross-checks the
    /// on-chain output against `foundry-evm-core`'s local precompile semantics.
    #[tokio::test]
    async fn live_precompile_probe_on_nile() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        use std::str::FromStr;
        let key = std::env::var("TRON_PRIVATE_KEY").expect("TRON_PRIVATE_KEY for live probe");
        let signer = alloy_signer_local::PrivateKeySigner::from_str(&key).unwrap();
        // nileex.io: nile.trongrid.io is unreachable from this host.
        let p = TronProvider::new("https://api.nileex.io").unwrap();

        // Generic STATICCALL proxy: runtime reads calldata word 0 as the target
        // address and forwards calldata[32..] to it, returning the raw output.
        let creation =
            hex::decode("601b8060095f395ff36020360360205f375f5f602036035f5f355afa503d5f5f3e3d5ff3")
                .unwrap();
        let opts = TxOptions { fee_limit: 400_000_000, expiration_ms: 60_000 };
        let poll = (30u32, Duration::from_secs(3));
        let (txid, proxy, info) =
            p.deploy_contract(&signer, creation, "PrecompileProxy", &opts, poll).await.unwrap();
        assert!(info.success, "proxy deploy must succeed");
        eprintln!(
            "live precompile proxy deploy tx {} -> {}",
            hex::encode(txid),
            foundry_tron_primitives::to_base58(proxy),
        );

        let target = |low: u8| {
            let mut w = [0u8; 32];
            w[31] = low;
            w
        };
        let owner = signer.address();

        // 0x03: sha256(sha256("abc")[..20]), not ripemd160("abc").
        let mut data03 = target(0x03).to_vec();
        data03.extend_from_slice(b"abc");
        let cr03 = p.trigger_constant(owner, proxy, &data03).await.unwrap();
        assert!(cr03.success, "0x03 staticcall must succeed");
        assert_eq!(
            hex::encode(&cr03.result),
            "6b6ea134869d649e6f52658be1a5691e37db83c6b8b72b0f1b36d4f849929c9e",
            "0x03 on Nile must be double-sha256, not ripemd160",
        );
        eprintln!("live 0x03(abc) = {}", hex::encode(&cr03.result));

        // 0x01: canonical ecrecover vector -> Tron 21-byte address form (byte 11 = 0x41).
        let ecrecover_input = hex::decode(
            "456e9aea5e197a1f1af7a3e85a3212fa4049a3ba34c2289b4c860fc0b0c64ef3\
             000000000000000000000000000000000000000000000000000000000000001c\
             9242685bf161793cc25603c231bc2f568eb630ea16aa137d2664ac8038825608\
             4f8ae3bd7535248d0bd448298cc2e2071e56992d0774dc340c368ae950852ada",
        )
        .unwrap();
        let mut data01 = target(0x01).to_vec();
        data01.extend_from_slice(&ecrecover_input);
        let cr01 = p.trigger_constant(owner, proxy, &data01).await.unwrap();
        assert!(cr01.success, "0x01 staticcall must succeed");
        assert_eq!(
            hex::encode(&cr01.result),
            "0000000000000000000000417156526fbd7a3c72969b54f64e42c10fbb768c8a",
            "0x01 on Nile must return the 21-byte (0x41-prefixed) address word",
        );
        eprintln!("live 0x01 ecrecover = {}", hex::encode(&cr01.result));
    }

    /// Live golden for the Tron CREATE2 scheme on Nile. Deploys the sandbox
    /// `Create2Factory`, then has the node compute both `childCodeHash()` (the
    /// exact embedded `Counter` init-code hash) and `deploy(salt)` (the CREATE2
    /// child address java-tron's `generateContractAddress2` produces, via a
    /// constant call that runs 0xF5 in simulation), and asserts the Rust
    /// [`foundry_tron_primitives::address::create2_address`] formula reproduces
    /// the node's address byte-for-byte. A final real `deploy(salt)` transaction
    /// confirms the child actually deploys on-chain at that address.
    #[tokio::test]
    async fn live_create2_golden_on_nile() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        use std::str::FromStr;
        let key = std::env::var("TRON_PRIVATE_KEY").expect("TRON_PRIVATE_KEY for live create2");
        let signer = alloy_signer_local::PrivateKeySigner::from_str(&key).unwrap();
        // nileex.io: nile.trongrid.io is unreachable from this host.
        let p = TronProvider::new("https://api.nileex.io").unwrap();

        // The sandbox `Create2Factory` (deploy(bytes32) + childCodeHash()),
        // compiled with tron-solc 0.8.27.
        let creation =
            hex::decode(include_str!("../testdata/tron_create2_factory_creation.hex").trim())
                .unwrap();
        let opts = TxOptions { fee_limit: 400_000_000, expiration_ms: 60_000 };
        let poll = (30u32, Duration::from_secs(3));

        let (txid, factory, info) =
            p.deploy_contract(&signer, creation, "Create2Factory", &opts, poll).await.unwrap();
        assert!(info.success, "factory deploy must succeed");
        eprintln!(
            "live create2 factory tx {} -> {}",
            hex::encode(txid),
            foundry_tron_primitives::to_base58(factory),
        );

        let owner = signer.address();
        let salt = B256::from(alloy_primitives::U256::from(0xC0FFEEu64));

        // childCodeHash(): the exact `Counter` init-code hash the node hashes.
        let cr_hash =
            p.trigger_constant(owner, factory, &hex::decode("ef803be1").unwrap()).await.unwrap();
        assert!(
            cr_hash.success && cr_hash.result.len() == 32,
            "childCodeHash() must return bytes32"
        );
        let init_code_hash = B256::from_slice(&cr_hash.result);

        // deploy(salt) as a constant call: the node runs CREATE2 and returns the
        // child address it computes (java-tron generateContractAddress2), without
        // persisting state.
        let mut deploy_data = hex::decode("2b85ba38").unwrap();
        deploy_data.extend_from_slice(salt.as_slice());
        let cr_addr = p.trigger_constant(owner, factory, &deploy_data).await.unwrap();
        assert!(cr_addr.success && cr_addr.result.len() == 32, "deploy() must return an address");
        let node_child = Address::from_slice(&cr_addr.result[12..]);

        // THE GOLDEN: the Rust formula must reproduce the node's CREATE2 address.
        let local_child =
            foundry_tron_primitives::address::create2_address(factory, salt, init_code_hash);
        assert_eq!(
            node_child,
            local_child,
            "Tron CREATE2 address mismatch: node {} != local {}",
            to_hex41(node_child),
            to_hex41(local_child),
        );
        eprintln!("live create2 child (node == local) = {}", to_hex41(node_child));

        // Confirm the child truly deploys on-chain at that address (real tx).
        let (deploy_txid, deploy_info) =
            p.trigger_contract(&signer, factory, 0, deploy_data, &opts, poll).await.unwrap();
        assert!(deploy_info.success, "on-chain CREATE2 deploy must succeed");
        eprintln!(
            "live create2 on-chain deploy tx https://nile.tronscan.org/#/transaction/{}",
            hex::encode(deploy_txid),
        );
    }

    // ---- golden energy parity harness (local tron-revm vs Nile node) ----

    /// A CANCUN environment mirroring the tron sandbox (`evm_version = "cancun"`),
    /// identical to the one the `foundry-evm-core` energy-model unit tests use, so
    /// local energy is metered under the same conditions the node runs under.
    fn tron_cancun_env() -> EvmEnv {
        let mut env: EvmEnv<SpecId> =
            EvmEnv { cfg_env: CfgEnv::new_with_spec(SpecId::CANCUN), ..Default::default() };
        env.block_env.gas_limit = 30_000_000;
        env.block_env.prevrandao = Some(B256::with_last_byte(0x11));
        env
    }

    /// Deploys the sandbox `Counter` creation bytecode through the local
    /// [`TronEvmFactory`] and returns the runtime code the constructor RETURNs --
    /// the exact bytes java-tron stores and later executes on a call.
    fn counter_runtime() -> Bytes {
        let creation = hex::decode(
            include_str!("../../../evm/core/testdata/tron_counter_creation.hex").trim(),
        )
        .unwrap();
        let mut evm = TronEvmFactory.create_evm(CacheDB::<EmptyDB>::default(), tron_cancun_env());
        let out = evm
            .transact_raw(TxEnv {
                kind: TxKind::Create,
                data: creation.into(),
                gas_limit: 10_000_000,
                ..Default::default()
            })
            .unwrap();
        assert!(out.result.is_success(), "counter must deploy locally: {:?}", out.result);
        out.result.output().unwrap().clone()
    }

    /// Runs `calldata` against a fresh account holding `runtime` on the local
    /// [`TronEvmFactory`] and returns the metered energy (`tx_gas_used`). The
    /// account starts with empty storage (slot 0 = 0), matching a freshly
    /// deployed Counter before its first `setNumber`. Intrinsic tx gas is zero
    /// under the Tron energy model (it is bandwidth, not energy), so this is pure
    /// execution energy -- the quantity the node reports as `energy_used`
    /// (constant call) / `energy_usage_total` (mined tx).
    fn local_call_energy(runtime: &Bytes, calldata: Vec<u8>) -> u64 {
        let contract = Address::from([0x42u8; 20]);
        let mut db = CacheDB::<EmptyDB>::default();
        db.insert_account_info(
            contract,
            AccountInfo::from_bytecode(Bytecode::new_raw(runtime.clone())),
        );
        let mut evm = TronEvmFactory.create_evm(db, tron_cancun_env());
        let out = evm
            .transact_raw(TxEnv {
                kind: TxKind::Call(contract),
                data: calldata.into(),
                gas_limit: 10_000_000,
                ..Default::default()
            })
            .unwrap();
        assert!(out.result.is_success(), "local call must succeed: {:?}", out.result);
        out.result.tx_gas_used()
    }

    /// GOLDEN energy parity against Nile: the local tron-revm energy model must
    /// reproduce the node's energy **exactly** for both a read and a write on a
    /// freshly deployed `Counter` (so the dynamic-energy factor is 1 and does not
    /// inflate the node's number). This is the end-to-end proof that the
    /// FRONTIER-table + TVM-delta energy model (plan E, Task 1) is faithful, not
    /// merely internally self-consistent: the local number is computed from the
    /// same runtime bytecode the node executes, and both are asserted equal with
    /// no tolerance. Energy (not `fee_sun`) is compared: staked resources zero the
    /// fee while energy is still metered.
    #[tokio::test]
    async fn live_golden_energy_parity_on_nile() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        use std::str::FromStr;
        let key = std::env::var("TRON_PRIVATE_KEY").expect("TRON_PRIVATE_KEY for live golden");
        let signer = alloy_signer_local::PrivateKeySigner::from_str(&key).unwrap();
        // nileex.io: nile.trongrid.io is unreachable from this host.
        let p = TronProvider::new("https://api.nileex.io").unwrap();
        let owner = signer.address();

        // Fresh Counter deploy (dynamic-energy factor = 1).
        let creation = hex::decode(
            include_str!("../../../evm/core/testdata/tron_counter_creation.hex").trim(),
        )
        .unwrap();
        let opts = TxOptions { fee_limit: 400_000_000, expiration_ms: 60_000 };
        let poll = (30u32, Duration::from_secs(3));
        let (deploy_txid, addr, info) =
            p.deploy_contract(&signer, creation, "Counter", &opts, poll).await.unwrap();
        assert!(info.success, "counter deploy must succeed");
        eprintln!(
            "golden fresh counter tx {} -> {}",
            hex::encode(deploy_txid),
            foundry_tron_primitives::to_base58(addr),
        );

        // The runtime the node stored == the constructor's local output.
        let runtime = counter_runtime();

        // ---- READ path: number() via triggerconstantcontract ----
        // Called before setNumber, so slot 0 is 0 both on-chain and locally.
        let number_sel = hex::decode("8381f58a").unwrap();
        let node_view = p.trigger_constant(owner, addr, &number_sel).await.unwrap();
        assert!(node_view.success, "number() constant call must succeed");
        let local_view = local_call_energy(&runtime, number_sel.clone());
        assert_eq!(
            local_view, node_view.energy_used,
            "READ energy parity failed: local tron-revm {local_view} != Nile {}",
            node_view.energy_used,
        );
        eprintln!("golden READ number(): local == node == {local_view} energy");

        // ---- WRITE path: setNumber(7) mined, TxInfo.energy_usage_total ----
        // First write on the fresh contract: slot 0 goes 0->7 = SSTORE SET (20000)
        // both on-chain and locally.
        let mut set = hex::decode("3fb5c1cb").unwrap();
        set.extend_from_slice(&U256::from(7u64).to_be_bytes::<32>());
        let (set_txid, set_info) =
            p.trigger_contract(&signer, addr, 0, set.clone(), &opts, poll).await.unwrap();
        assert!(set_info.success, "setNumber(7) must succeed");
        let local_write = local_call_energy(&runtime, set);
        assert_eq!(
            local_write, set_info.energy_used,
            "WRITE energy parity failed: local tron-revm {local_write} != Nile {}",
            set_info.energy_used,
        );
        eprintln!(
            "golden WRITE setNumber(7): local == node == {local_write} energy (tx {})",
            hex::encode(set_txid),
        );
    }
}
