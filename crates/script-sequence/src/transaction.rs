use alloy_network::Network;
use alloy_primitives::{Address, B256, Bytes};
use foundry_common::TransactionMaybeSigned;
use revm_inspectors::tracing::types::CallKind;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdditionalContract {
    #[serde(rename = "transactionType")]
    pub call_kind: CallKind,
    pub contract_name: Option<String>,
    pub address: Address,
    pub init_code: Bytes,
}

/// Tron-specific metadata attached to a broadcasted transaction.
///
/// Tron transactions are protobuf, identified by `txid = sha256(raw_data)` and broadcast through
/// the HTTP `/wallet/*` API, so they carry no EVM receipt. This section records the fields the EVM
/// broadcast artifact cannot express: the base58 forms of the owner and the deployed contract, the
/// `fee_limit` cap and the confirmed energy/fee. The base58 strings are prepared by the `forge
/// script` Tron path; this crate stays free of any dependency on the Tron crates. The section is
/// optional and skipped when absent, so existing Ethereum broadcast artifacts keep deserializing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TronTxMeta {
    /// `txid = sha256(raw_data)`, the Tron transaction id (mirrors `hash`).
    pub txid: B256,
    /// Sender (owner) address in base58check (`T…`) form.
    pub owner_base58: String,
    /// Deployed contract address in base58check form, for deploys only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_address_base58: Option<String>,
    /// The `fee_limit` (SUN) the transaction was signed with.
    pub fee_limit: i64,
    /// Energy consumed by the transaction, once confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_used: Option<u64>,
    /// Fee burned by the transaction in SUN, once confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_sun: Option<u64>,
    /// Absolute expiration of the signed transaction (unix ms), recorded before broadcast and
    /// cleared once confirmed. On `--resume` it bounds how long a persisted-but-unconfirmed txid
    /// is polled before a rebuild is safe: Tron has no nonce, so a rebuilt transaction gets a
    /// new txid the network treats as independent, and rebuilding while the original is still
    /// valid would double-execute. Present only for an in-flight (pending) transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiration_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    bound(
        serialize = "N::TransactionRequest: Serialize, N::TxEnvelope: Serialize",
        deserialize = "N::TransactionRequest: for<'de2> Deserialize<'de2>, N::TxEnvelope: for<'de2> Deserialize<'de2>"
    )
)]
pub struct TransactionWithMetadata<N: Network> {
    pub hash: Option<B256>,
    #[serde(rename = "transactionType")]
    pub call_kind: CallKind,
    #[serde(default = "default_string")]
    pub contract_name: Option<String>,
    #[serde(default = "default_address")]
    pub contract_address: Option<Address>,
    #[serde(default = "default_string")]
    pub function: Option<String>,
    #[serde(default = "default_vec_of_strings")]
    pub arguments: Option<Vec<String>>,
    #[serde(skip)]
    pub rpc: String,
    pub transaction: TransactionMaybeSigned<N>,
    #[serde(default)]
    pub additional_contracts: Vec<AdditionalContract>,
    #[serde(default)]
    pub is_fixed_gas_limit: bool,
    /// Tron-specific metadata, present only for transactions broadcast on Tron
    /// (`network = "tron"`). Optional so Ethereum broadcast artifacts keep deserializing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tron: Option<TronTxMeta>,
}

const fn default_string() -> Option<String> {
    Some(String::new())
}

const fn default_address() -> Option<Address> {
    Some(Address::ZERO)
}

const fn default_vec_of_strings() -> Option<Vec<String>> {
    Some(vec![])
}

impl<N: Network> TransactionWithMetadata<N> {
    pub fn from_tx_request(transaction: TransactionMaybeSigned<N>) -> Self {
        Self {
            transaction,
            hash: Default::default(),
            call_kind: Default::default(),
            contract_name: Default::default(),
            contract_address: Default::default(),
            function: Default::default(),
            arguments: Default::default(),
            is_fixed_gas_limit: Default::default(),
            additional_contracts: Default::default(),
            rpc: Default::default(),
            tron: Default::default(),
        }
    }

    pub const fn tx(&self) -> &TransactionMaybeSigned<N> {
        &self.transaction
    }

    pub const fn tx_mut(&mut self) -> &mut TransactionMaybeSigned<N> {
        &mut self.transaction
    }

    pub fn is_create2(&self) -> bool {
        self.call_kind == CallKind::Create2
    }
}
