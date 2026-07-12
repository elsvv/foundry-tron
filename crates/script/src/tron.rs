//! Tron broadcast path for `forge script` (`network = "tron"`).
//!
//! Tron transactions are protobuf, identified by `txid = sha256(raw_data)`, signed over that hash
//! and broadcast through the HTTP `/wallet/*` API rather than `eth_sendRawTransaction`. The generic
//! [`BundledState::broadcast`](crate::broadcast::BundledState::broadcast) alloy-send path therefore
//! does not apply; [`BundledState<TronEvmNetwork>::broadcast_tron`] takes over instead.
//!
//! Simulation is shared with every other network: the script runs locally on the `TronEvmFactory`
//! and the collected transactions arrive here as a [`ScriptSequence`](forge_script_sequence). This
//! module walks that sequence sequentially, one fresh TAPOS reference block per transaction, builds
//! and signs the matching protobuf contract, broadcasts it and waits for confirmation. Deploy
//! addresses are derived locally from the txid (java-tron `WalletUtil` formula) because Tron has no
//! CREATE nonce scheme, and are cross-checked against the address the node reports once mined.
//!
//! `--resume` picks up a run that was interrupted mid-broadcast. Because Tron has no nonce, a
//! rebuilt transaction gets a fresh txid the network treats as independent, so re-sending one while
//! the original is still valid (up to its ~60s expiration) would double-execute. To close that
//! window, each txid and its expiration are checkpointed *before* the broadcast call; on resume a
//! recorded-but-unconfirmed txid is polled ([`adopt_recorded`]) and only rebuilt once its recorded
//! expiration has provably passed unconfirmed.

use crate::{broadcast::BundledState, sequence::ScriptSequenceKind, verify::BroadcastedState};
use alloy_consensus::{Eip658Value, Receipt, ReceiptEnvelope, ReceiptWithBloom};
use alloy_network::Ethereum;
use alloy_primitives::{Address, B256, Bloom, TxKind, U256, hex};
use alloy_rpc_types::TransactionReceipt;
use alloy_signer::Signer;
use eyre::{Result, bail};
use forge_script_sequence::{TransactionWithMetadata, TronTxMeta};
use foundry_common::shell;
use foundry_config::TronConfig;
use foundry_evm::core::evm::TronEvmNetwork;
use foundry_tron_primitives::{
    address::contract_address_from_txid, sign::sign_raw_with, to_base58, to_hex41,
    units::format_sun_as_trx,
};
use foundry_tron_provider::{
    TronError, TronProvider, TxInfo, TxOptions, build_create_raw, build_transfer_raw,
    build_trigger_raw,
};
use foundry_wallets::WalletSigner;
use std::{
    collections::HashMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Environment variable carrying the TronGrid API key (forwarded as `TRON-PRO-API-KEY`).
const TRON_API_KEY_ENV: &str = "TRON_PRO_API_KEY";

/// Poll interval, in seconds, while waiting for confirmation.
const POLL_INTERVAL_SECS: u64 = 3;

/// Maximum number of TAPOS rebuilds when a transaction expires unconfirmed (spec §4.5/§8).
const MAX_REBUILDS: u8 = 2;

/// Maximum retries for a transient HTTP failure (dropped connection, request timeout) before
/// giving up on a single request.
const MAX_HTTP_RETRIES: u8 = 3;

/// A single Tron transaction classified from a script broadcastable transaction.
#[derive(Debug, Clone)]
enum TronCall {
    /// Contract deployment: creation bytecode followed by ABI-encoded constructor args.
    Create { code: Vec<u8>, value: i64 },
    /// Contract call: destination plus ABI-encoded calldata.
    Trigger { to: Address, data: Vec<u8>, value: i64 },
    /// Native TRX transfer (empty calldata).
    Transfer { to: Address, value: i64 },
}

/// Outcome of broadcasting and confirming one Tron transaction.
struct SentTx {
    txid: B256,
    info: TxInfo,
    /// Deployed contract address (for `Create` only), derived from the txid and cross-checked.
    contract_address: Option<Address>,
    fee_limit: i64,
}

impl BundledState<TronEvmNetwork> {
    /// Broadcasts every transaction in the bundled sequence over Tron's `/wallet/*` API.
    ///
    /// Sequential, one transaction at a time with a fresh TAPOS reference block per transaction.
    /// A deploy's address is derived locally from its txid and, together with subsequent calls to
    /// it, remapped from the EVM simulation address to the real Tron address. Every transaction is
    /// confirmed before the next is sent; an expired-but-unconfirmed transaction is rebuilt with a
    /// fresh TAPOS (new txid) up to [`MAX_REBUILDS`] times. Receipts are synthesized from the
    /// node's `gettransactioninfobyid` (energy → `gas_used`, fee reported separately in TRX).
    pub async fn broadcast_tron(self) -> Result<BroadcastedState<TronEvmNetwork>> {
        let Self {
            args,
            script_config,
            script_wallets,
            browser_wallet: _,
            build_data,
            mut sequence,
        } = self;

        // Library predeploys become their own `Create` transactions with an EVM-scheme address the
        // rest of the script links against. Remapping those to the txid-derived Tron address after
        // the fact would desync every already-linked reference, so predeploys are out of scope for
        // this stage. Counter needs none; a script that does is rejected up front.
        if build_data.predeploy_libraries.libraries_count() > 0 {
            bail!("library predeploys on tron: stage 2");
        }

        // Tron uses the async `sign_hash` path, so every WalletSigner backend works.
        let signers = script_wallets
            .into_multi_wallet()
            .into_signers()
            .map_err(|e| eyre::eyre!("failed to resolve wallet signers: {e}"))?;

        let tron_cfg = args.tron.apply(&script_config.config.tron);
        let opts = tx_options(&tron_cfg);
        let (attempts, interval) = poll_params(&tron_cfg);

        for i in 0..sequence.sequences().len() {
            let rpc = sequence.sequences()[i].rpc_url().to_string();
            let chain = sequence.sequences()[i].chain;
            let provider = build_provider(&rpc)?;

            let already = sequence.sequences()[i].receipts.len();
            let total = sequence.sequences()[i].transactions.len();
            if already >= total {
                continue;
            }

            if !shell::is_json() {
                sh_println!(
                    "\n## Broadcasting {} transaction(s) to Tron chain {chain}...",
                    total - already
                )?;
            }

            // Maps an EVM-simulation deploy address to the real on-chain Tron address, so calls to
            // a just-deployed contract are retargeted from the simulated address to the real one.
            let mut remap: HashMap<Address, Address> = HashMap::new();

            for index in already..total {
                let (call, sim_addr, to_for_receipt, from) =
                    classify(&sequence.sequences()[i].transactions[index], &remap, &script_config)?;

                let signer = signers.get(&from).ok_or_else(|| {
                    eyre::eyre!("no wallet signer available for tron sender {from:#x}")
                })?;
                let owner = signer.address();

                // A prior run may have checkpointed a txid for this index before crashing between
                // broadcast and confirmation. Adopt that exact txid rather than build a competing
                // one; only fall through to a fresh build once it has provably expired unconfirmed.
                let recorded = sequence.sequences()[i].transactions[index].tron.clone();
                let sent = match recorded {
                    Some(prev) => {
                        match adopt_recorded(&provider, &prev, &call, owner, interval).await? {
                            Some(sent) => sent,
                            None => {
                                broadcast_fresh(
                                    &mut sequence,
                                    i,
                                    index,
                                    &provider,
                                    signer,
                                    owner,
                                    &call,
                                    &opts,
                                    attempts,
                                    interval,
                                    tron_cfg.origin_energy_limit,
                                    tron_cfg.user_fee_percentage,
                                )
                                .await?
                            }
                        }
                    }
                    None => {
                        broadcast_fresh(
                            &mut sequence,
                            i,
                            index,
                            &provider,
                            signer,
                            owner,
                            &call,
                            &opts,
                            attempts,
                            interval,
                            tron_cfg.origin_energy_limit,
                            tron_cfg.user_fee_percentage,
                        )
                        .await?
                    }
                };

                report_progress(&sent)?;

                let contract_address = sent.contract_address;
                {
                    let seq = &mut sequence.sequences_mut()[i];
                    let meta = &mut seq.transactions[index];
                    meta.hash = Some(sent.txid);
                    if let Some(real) = contract_address {
                        // Record the simulation → on-chain remap and overwrite the EVM-scheme
                        // address the simulation guessed with the true Tron deploy address.
                        if let Some(sim) = sim_addr {
                            remap.insert(sim, real);
                        }
                        meta.contract_address = Some(real);
                    } else if let Some(real_to) = to_for_receipt {
                        // Call/transfer: `to` may have been remapped from a simulation deploy
                        // address to the real on-chain one. Rewrite the recorded target and the
                        // request's `to` so the artifact is honest and a later `--resume`
                        // re-classifies straight to the real address (the in-memory remap, rebuilt
                        // per run, is empty then).
                        meta.contract_address = Some(real_to);
                        if let Some(req) = meta.transaction.as_unsigned_mut() {
                            req.to = Some(TxKind::Call(real_to));
                        }
                    }
                    // Confirmed: drop the expiration (its only use is bounding a pending resume)
                    // and clear the pending marker now that a receipt records
                    // the outcome.
                    meta.tron = Some(TronTxMeta {
                        txid: sent.txid,
                        owner_base58: to_base58(owner),
                        contract_address_base58: contract_address.map(to_base58),
                        fee_limit: sent.fee_limit,
                        energy_used: Some(sent.info.energy_used),
                        fee_sun: Some(sent.info.fee_sun),
                        expiration_ms: None,
                    });
                    seq.remove_pending(sent.txid);
                    seq.receipts.push(tron_receipt(
                        sent.txid,
                        owner,
                        &sent.info,
                        contract_address,
                        to_for_receipt,
                        index,
                    ));
                }
                // Checkpoint so a mid-run failure can be inspected/resumed.
                sequence.save(true, false)?;
            }
        }

        if !shell::is_json() {
            sh_println!("\n\n==========================")?;
            sh_println!("\nTRON EXECUTION COMPLETE & SUCCESSFUL.")?;
        }

        Ok(BroadcastedState { args, script_config, build_data, sequence })
    }
}

/// Classifies a script transaction into the Tron contract to build, resolving the deploy-address
/// remap for calls to just-deployed contracts. Returns the call, the simulation contract address
/// (deploy remap key), the receipt `to` and the resolved sender.
fn classify(
    tx: &TransactionWithMetadata<Ethereum>,
    remap: &HashMap<Address, Address>,
    script_config: &crate::ScriptConfig<TronEvmNetwork>,
) -> Result<(TronCall, Option<Address>, Option<Address>, Address)> {
    let inner = tx.tx();
    let from = inner.from().unwrap_or(script_config.evm_opts.sender);
    let value = value_to_sun(inner.value())?;

    let (call, to_for_receipt) = match inner.to() {
        None => {
            let code = inner.input().map(|b| b.to_vec()).unwrap_or_default();
            if code.is_empty() {
                bail!("tron deploy transaction has empty creation bytecode");
            }
            (TronCall::Create { code, value }, None)
        }
        Some(to) => {
            let to = remap.get(&to).copied().unwrap_or(to);
            let data = inner.input().map(|b| b.to_vec()).unwrap_or_default();
            if data.is_empty() {
                (TronCall::Transfer { to, value }, Some(to))
            } else {
                (TronCall::Trigger { to, data, value }, Some(to))
            }
        }
    };

    Ok((call, tx.contract_address, to_for_receipt, from))
}

/// Extra margin (ms) past a recorded expiration before a pending transaction is treated as
/// unminable. Tron validity is decided against block time; this absorbs wall-clock vs block-time
/// skew so a rebuild never races a transaction that could still be mined.
const EXPIRY_SKEW_MS: i64 = 5_000;

/// Builds, signs, persists (before broadcast), broadcasts and confirms one fresh Tron transaction
/// for `seq.sequences()[i].transactions[index]`, rebuilding with a fresh TAPOS on expiry up to
/// [`MAX_REBUILDS`] times. For deploys, derives the address locally and cross-checks it against the
/// node's reported `contract_address`.
///
/// The txid and its expiration are checkpointed to the sequence *before* the broadcast call. Tron
/// has no nonce, so if a crash lands between broadcast and confirmation and the txid were not
/// recorded, a later `--resume` would build a fresh transaction with a new txid and re-send it
/// while the original is still valid — a double execution. Recording first lets resume adopt the
/// exact txid instead (see [`adopt_recorded`]).
#[allow(clippy::too_many_arguments)]
async fn broadcast_fresh(
    seq: &mut ScriptSequenceKind<Ethereum>,
    i: usize,
    index: usize,
    provider: &TronProvider,
    signer: &WalletSigner,
    owner: Address,
    call: &TronCall,
    opts: &TxOptions,
    attempts: u32,
    interval: Duration,
    origin_energy_limit: i64,
    user_fee_percentage: i64,
) -> Result<SentTx> {
    let mut rebuilds = 0u8;
    // txid of the previous, now-expired attempt (rebuild only), cleared from `pending` on rebuild.
    let mut prev_txid: Option<B256> = None;
    loop {
        let (rb, now_ms) = tapos_with_retry(provider).await?;
        let raw = match call {
            TronCall::Create { code, value } => build_create_raw(
                owner,
                code.clone(),
                "",
                *value,
                origin_energy_limit,
                user_fee_percentage,
                rb,
                now_ms,
                opts,
            ),
            TronCall::Trigger { to, data, value } => {
                build_trigger_raw(owner, *to, *value, data.clone(), rb, now_ms, opts)
            }
            TronCall::Transfer { to, value } => {
                build_transfer_raw(owner, *to, *value, rb, now_ms, opts)
            }
        };
        let expiration_ms = raw.expiration;

        let signed = sign_raw_with(raw, signer)
            .await
            .map_err(|e| eyre::eyre!("failed to sign tron transaction: {e}"))?;
        let local_addr = matches!(call, TronCall::Create { .. })
            .then(|| contract_address_from_txid(signed.txid, owner));

        // Record the txid + expiration and checkpoint BEFORE broadcasting, so a crash in the
        // broadcast/confirm window leaves a record `--resume` can adopt.
        {
            let s = &mut seq.sequences_mut()[i];
            if let Some(old) = prev_txid.take() {
                // Superseded by this rebuild; the old txid has expired and can never confirm.
                s.remove_pending(old);
            }
            s.transactions[index].tron = Some(TronTxMeta {
                txid: signed.txid,
                owner_base58: to_base58(owner),
                contract_address_base58: local_addr.map(to_base58),
                fee_limit: opts.fee_limit,
                energy_used: None,
                fee_sun: None,
                expiration_ms: Some(expiration_ms),
            });
            s.add_pending(index, signed.txid);
        }
        seq.save(true, false)?;
        prev_txid = Some(signed.txid);

        broadcast_with_retry(provider, &signed).await?;

        match provider.wait_for_confirmation(signed.txid, attempts, interval).await {
            Ok(info) => {
                if !info.success {
                    bail!("tron transaction {} reverted on chain", hex::encode(signed.txid));
                }
                if let Some(local) = local_addr {
                    verify_deploy_address(local, &info)?;
                }
                return Ok(SentTx {
                    txid: signed.txid,
                    info,
                    contract_address: local_addr,
                    fee_limit: opts.fee_limit,
                });
            }
            // Confirmation window (attempts * interval) covers expiration + margin, so a timeout
            // means the transaction expired unmined: rebuild with a fresh TAPOS (new txid).
            Err(TronError::Timeout(_)) if rebuilds < MAX_REBUILDS => {
                rebuilds += 1;
                sh_warn!(
                    "tron tx {} expired unconfirmed; rebuilding with fresh TAPOS (rebuild {}/{})",
                    hex::encode(signed.txid),
                    rebuilds,
                    MAX_REBUILDS
                )?;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// On `--resume`, decides the fate of a txid a prior run checkpointed for this index before it was
/// confirmed.
///
/// Polls `gettransactioninfobyid` for the recorded txid and returns `Some(SentTx)` the moment it
/// confirms. Returns `None` only once the recorded expiration has provably passed without
/// confirmation — the single point at which the transaction can no longer be mined and rebuilding a
/// fresh txid is safe. While still inside its expiration window it keeps polling rather than
/// rebuild: re-broadcasting the same txid would be redundant (Tron dedupes it), and building a new
/// one could double-execute. For deploys the address is re-derived from the txid and cross-checked,
/// exactly as on the fresh path.
async fn adopt_recorded(
    provider: &TronProvider,
    prev: &TronTxMeta,
    call: &TronCall,
    owner: Address,
    interval: Duration,
) -> Result<Option<SentTx>> {
    loop {
        match provider.get_transaction_info(prev.txid).await {
            Ok(Some(info)) => {
                if !info.success {
                    bail!("resumed tron tx {} reverted on chain", hex::encode(prev.txid));
                }
                let local_addr = matches!(call, TronCall::Create { .. })
                    .then(|| contract_address_from_txid(prev.txid, owner));
                if let Some(local) = local_addr {
                    verify_deploy_address(local, &info)?;
                }
                return Ok(Some(SentTx {
                    txid: prev.txid,
                    info,
                    contract_address: local_addr,
                    fee_limit: prev.fee_limit,
                }));
            }
            Ok(None) => {}
            // Transient HTTP hiccup: treat like a still-pending poll and retry within the loop.
            Err(TronError::Http(_)) => {}
            Err(e) => return Err(e.into()),
        }
        // Not confirmed yet. Once the recorded window has closed the txid can never mine, so a
        // rebuild is finally safe. Checked after every non-confirming poll (including HTTP errors)
        // so a dead endpoint past expiration still terminates.
        if is_expired(prev.expiration_ms) {
            return Ok(None);
        }
        tokio::time::sleep(interval).await;
    }
}

/// Returns whether `expiration_ms` (absolute unix ms) has passed by more than [`EXPIRY_SKEW_MS`].
///
/// A missing expiration reads as expired: only [`broadcast_fresh`] writes a pending record and it
/// always sets the field, so `None` here means a legacy or hand-edited artifact that carries no
/// proof the txid is still valid — with no window to wait out, rebuilding is the only way to make
/// progress. Callers reach this only after the node reported no receipt for the txid.
fn is_expired(expiration_ms: Option<i64>) -> bool {
    let Some(exp) = expiration_ms else { return true };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(i64::MAX);
    now_ms > exp + EXPIRY_SKEW_MS
}

/// Fetches TAPOS, retrying transient HTTP failures with a short backoff.
async fn tapos_with_retry(
    provider: &TronProvider,
) -> Result<(foundry_tron_primitives::RefBlock, i64)> {
    let mut attempt = 0u8;
    loop {
        match provider.tapos().await {
            Ok(v) => return Ok(v),
            Err(e @ TronError::Http(_)) if attempt < MAX_HTTP_RETRIES => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                let _ = e;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Broadcasts a signed transaction, retrying transient HTTP failures with a short backoff.
async fn broadcast_with_retry(
    provider: &TronProvider,
    signed: &foundry_tron_primitives::SignedTronTx,
) -> Result<()> {
    let mut attempt = 0u8;
    loop {
        match provider.broadcast(signed).await {
            Ok(()) => return Ok(()),
            Err(TronError::Http(_)) if attempt < MAX_HTTP_RETRIES => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Cross-checks the locally derived deploy address against the node's `contract_address`.
fn verify_deploy_address(local: Address, info: &TxInfo) -> Result<()> {
    match info.contract_address {
        Some(node) if node == local => Ok(()),
        Some(node) => bail!(
            "tron deploy address mismatch: local {} != node {}",
            to_hex41(local),
            to_hex41(node)
        ),
        None => bail!(
            "node reported no contract_address for the deploy (local {}); the deploy may have failed",
            to_hex41(local)
        ),
    }
}

/// Synthesizes an Ethereum-shaped receipt from Tron transaction info: energy maps to `gas_used`,
/// `effective_gas_price` is zero (fees are reported separately in TRX), and there are no EVM logs.
const fn tron_receipt(
    txid: B256,
    from: Address,
    info: &TxInfo,
    contract_address: Option<Address>,
    to: Option<Address>,
    index: usize,
) -> TransactionReceipt {
    TransactionReceipt {
        inner: ReceiptEnvelope::Legacy(ReceiptWithBloom {
            receipt: Receipt {
                status: Eip658Value::Eip658(info.success),
                cumulative_gas_used: info.energy_used,
                logs: vec![],
            },
            logs_bloom: Bloom::ZERO,
        }),
        transaction_hash: txid,
        transaction_index: Some(index as u64),
        block_hash: None,
        block_number: Some(info.block_number as u64),
        gas_used: info.energy_used,
        effective_gas_price: 0,
        blob_gas_used: None,
        blob_gas_price: None,
        from,
        to,
        contract_address,
    }
}

/// Prints Tron transaction progress (energy and TRX fee, not ETH/gas) to stderr.
fn report_progress(sent: &SentTx) -> Result<()> {
    sh_status!(
        "tron: {} confirmed ({} TRX fee, {} energy)",
        hex::encode(sent.txid),
        format_sun_as_trx(sent.info.fee_sun),
        sent.info.energy_used
    )?;
    if let Some(addr) = sent.contract_address {
        sh_status!("tron: deployed to {} ({})", to_base58(addr), to_hex41(addr))?;
    }
    Ok(())
}

/// Resolves the Tron chain id for `rpc` (used to name the `broadcast/<script>/<chain>/`
/// directory). Reads `eth_chainId` from the node's `/jsonrpc` endpoint via [`TronProvider`], which
/// targets the correct path (the wallet base rejects JSON-RPC).
pub(crate) async fn tron_chain_id(rpc: &str) -> Result<u64> {
    let provider = build_provider(rpc)?;
    provider.get_chain_id().await.map_err(Into::into)
}

/// Builds a [`TronProvider`] for `rpc`, attaching the `TRON_PRO_API_KEY` header when present.
fn build_provider(rpc: &str) -> Result<TronProvider> {
    let mut provider = TronProvider::new(rpc)?;
    if let Ok(key) = std::env::var(TRON_API_KEY_ENV)
        && !key.is_empty()
    {
        provider = provider.with_api_key(key);
    }
    Ok(provider)
}

/// Interprets a transaction `value` (denominated in SUN on Tron) as an `i64` amount.
fn value_to_sun(value: Option<U256>) -> Result<i64> {
    let v = value.unwrap_or(U256::ZERO);
    let as_u64 = u64::try_from(v).map_err(|_| eyre::eyre!("value exceeds u64 SUN"))?;
    i64::try_from(as_u64).map_err(|_| eyre::eyre!("value exceeds i64 SUN"))
}

/// Converts a [`TronConfig`] into [`TxOptions`]: `fee_limit` stays in SUN, `expiration` seconds
/// become milliseconds.
const fn tx_options(cfg: &TronConfig) -> TxOptions {
    TxOptions { fee_limit: cfg.fee_limit, expiration_ms: (cfg.expiration as i64) * 1000 }
}

/// Derives `(attempts, interval)` covering the expiration window plus a 30s margin at a fixed 3s
/// interval, so confirmation polling never gives up before a still-valid transaction can be mined.
fn poll_params(cfg: &TronConfig) -> (u32, Duration) {
    let attempts = (cfg.expiration + 30).div_ceil(POLL_INTERVAL_SECS).max(1) as u32;
    (attempts, Duration::from_secs(POLL_INTERVAL_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Bytes, address};
    use alloy_rpc_types::TransactionRequest;
    use foundry_common::TransactionMaybeSigned;

    fn tx_with(req: TransactionRequest) -> TransactionWithMetadata<Ethereum> {
        TransactionWithMetadata::from_tx_request(TransactionMaybeSigned::new(req))
    }

    fn script_config() -> crate::ScriptConfig<TronEvmNetwork> {
        // A cheap default; only `evm_opts.sender` is read by `classify`.
        crate::ScriptConfig {
            config: Default::default(),
            evm_opts: Default::default(),
            sender_nonce: 1,
            backends: Default::default(),
            batch: false,
            tempo: Default::default(),
        }
    }

    #[test]
    fn classify_create_from_missing_to() {
        let code = Bytes::from(vec![0x60, 0x80, 0x60, 0x40]);
        let tx = tx_with(TransactionRequest {
            from: Some(address!("0x1111111111111111111111111111111111111111")),
            to: None,
            input: code.clone().into(),
            ..Default::default()
        });
        let (call, _, to_for_receipt, from) =
            classify(&tx, &HashMap::new(), &script_config()).unwrap();
        assert!(matches!(call, TronCall::Create { code: c, value: 0 } if c == code.to_vec()));
        assert_eq!(to_for_receipt, None);
        assert_eq!(from, address!("0x1111111111111111111111111111111111111111"));
    }

    #[test]
    fn classify_trigger_from_calldata() {
        let to = address!("0x2222222222222222222222222222222222222222");
        let data = Bytes::from(hex::decode("3fb5c1cb").unwrap());
        let tx = tx_with(TransactionRequest {
            from: Some(address!("0x1111111111111111111111111111111111111111")),
            to: Some(to.into()),
            input: data.into(),
            ..Default::default()
        });
        let (call, _, to_for_receipt, _) =
            classify(&tx, &HashMap::new(), &script_config()).unwrap();
        assert!(matches!(call, TronCall::Trigger { to: t, .. } if t == to));
        assert_eq!(to_for_receipt, Some(to));
    }

    #[test]
    fn classify_transfer_from_empty_calldata() {
        let to = address!("0x2222222222222222222222222222222222222222");
        let tx = tx_with(TransactionRequest {
            from: Some(address!("0x1111111111111111111111111111111111111111")),
            to: Some(to.into()),
            value: Some(U256::from(1_000_000u64)),
            ..Default::default()
        });
        let (call, ..) = classify(&tx, &HashMap::new(), &script_config()).unwrap();
        assert!(matches!(call, TronCall::Transfer { to: t, value: 1_000_000 } if t == to));
    }

    #[test]
    fn classify_remaps_call_target_to_real_address() {
        let sim = address!("0x2222222222222222222222222222222222222222");
        let real = address!("0x3333333333333333333333333333333333333333");
        let mut remap = HashMap::new();
        remap.insert(sim, real);
        let tx = tx_with(TransactionRequest {
            from: Some(address!("0x1111111111111111111111111111111111111111")),
            to: Some(sim.into()),
            input: Bytes::from(hex::decode("3fb5c1cb").unwrap()).into(),
            ..Default::default()
        });
        let (call, _, to_for_receipt, _) = classify(&tx, &remap, &script_config()).unwrap();
        assert!(matches!(call, TronCall::Trigger { to: t, .. } if t == real));
        assert_eq!(to_for_receipt, Some(real));
    }

    #[test]
    fn value_to_sun_bounds() {
        assert_eq!(value_to_sun(None).unwrap(), 0);
        assert_eq!(value_to_sun(Some(U256::from(1_500_000u64))).unwrap(), 1_500_000);
        assert!(value_to_sun(Some(U256::MAX)).is_err());
    }

    #[test]
    fn tx_options_and_poll_params_from_config() {
        let cfg = TronConfig { fee_limit: 400_000_000, expiration: 30, ..TronConfig::default() };
        let opts = tx_options(&cfg);
        assert_eq!(opts.fee_limit, 400_000_000);
        assert_eq!(opts.expiration_ms, 30_000);
        let (attempts, interval) = poll_params(&cfg);
        assert_eq!(interval, Duration::from_secs(3));
        assert_eq!(attempts, 20); // (30 + 30) / 3
        assert!((attempts as u64) * interval.as_secs() >= cfg.expiration + 30);
    }

    #[test]
    fn tron_receipt_maps_energy_to_gas() {
        let txid = B256::repeat_byte(0xab);
        let from = address!("0x1111111111111111111111111111111111111111");
        let contract = address!("0x3333333333333333333333333333333333333333");
        let info = TxInfo {
            block_number: 69_000_000,
            fee_sun: 1_100_000,
            energy_used: 31_895,
            success: true,
            contract_address: Some(contract),
        };
        let r = tron_receipt(txid, from, &info, Some(contract), None, 0);
        assert_eq!(r.gas_used, 31_895);
        assert_eq!(r.effective_gas_price, 0);
        assert_eq!(r.contract_address, Some(contract));
        assert_eq!(r.block_number, Some(69_000_000));
        assert_eq!(r.transaction_hash, txid);
    }

    #[test]
    fn is_expired_uses_recorded_expiration_and_skew() {
        let now_ms =
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap();
        // Well in the future: still valid, must not rebuild.
        assert!(!is_expired(Some(now_ms + 60_000)));
        // Just past expiration but inside the skew margin: still treated as valid.
        assert!(!is_expired(Some(now_ms - EXPIRY_SKEW_MS / 2)));
        // Comfortably past expiration + skew: expired, safe to rebuild.
        assert!(is_expired(Some(now_ms - EXPIRY_SKEW_MS - 60_000)));
        // No recorded window: cannot wait it out, so rebuild.
        assert!(is_expired(None));
    }

    #[test]
    fn verify_deploy_address_mismatch_is_error() {
        let local = address!("0x3333333333333333333333333333333333333333");
        let other = address!("0x4444444444444444444444444444444444444444");
        let ok = TxInfo {
            block_number: 1,
            fee_sun: 0,
            energy_used: 0,
            success: true,
            contract_address: Some(local),
        };
        assert!(verify_deploy_address(local, &ok).is_ok());
        let bad = TxInfo { contract_address: Some(other), ..ok };
        assert!(verify_deploy_address(local, &bad).is_err());
        let none = TxInfo { contract_address: None, ..ok };
        assert!(verify_deploy_address(local, &none).is_err());
    }
}
