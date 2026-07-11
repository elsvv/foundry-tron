//! Hand-written prost mappings of the Tron protocol messages we need.
//! Field numbers mirror tronprotocol/protocol; correctness is proven by
//! the mainnet fixture round-trip test below.

use alloy_primitives::B256;
use prost::Message;
use sha2::{Digest, Sha256};

/// `Transaction.raw` from `core/Tron.proto` — the signed payload whose
/// sha256 is the transaction id.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TransactionRaw {
    #[prost(bytes = "vec", tag = "1")]
    pub ref_block_bytes: Vec<u8>,
    #[prost(int64, tag = "3")]
    pub ref_block_num: i64,
    #[prost(bytes = "vec", tag = "4")]
    pub ref_block_hash: Vec<u8>,
    #[prost(int64, tag = "8")]
    pub expiration: i64,
    #[prost(bytes = "vec", tag = "10")]
    pub data: Vec<u8>,
    #[prost(message, repeated, tag = "11")]
    pub contract: Vec<Contract>,
    #[prost(bytes = "vec", tag = "12")]
    pub scripts: Vec<u8>,
    #[prost(int64, tag = "14")]
    pub timestamp: i64,
    #[prost(int64, tag = "18")]
    pub fee_limit: i64,
}

/// `Transaction.Contract` from `core/Tron.proto`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Contract {
    #[prost(int32, tag = "1")]
    pub r#type: i32,
    #[prost(message, optional, tag = "2")]
    pub parameter: Option<prost_types::Any>,
    #[prost(bytes = "vec", tag = "3")]
    pub provider: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    pub contract_name: Vec<u8>,
    #[prost(int32, tag = "5")]
    pub permission_id: i32,
}

/// `Transaction` from `core/Tron.proto`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Transaction {
    #[prost(message, optional, tag = "1")]
    pub raw_data: Option<TransactionRaw>,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub signature: Vec<Vec<u8>>,
}

/// `TriggerSmartContract` from `core/contract/smart_contract.proto`.
#[derive(Clone, PartialEq, Eq, ::prost::Message)]
pub struct TriggerSmartContract {
    #[prost(bytes = "vec", tag = "1")]
    pub owner_address: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub contract_address: Vec<u8>,
    #[prost(int64, tag = "3")]
    pub call_value: i64,
    #[prost(bytes = "vec", tag = "4")]
    pub data: Vec<u8>,
    #[prost(int64, tag = "5")]
    pub call_token_value: i64,
    #[prost(int64, tag = "6")]
    pub token_id: i64,
}

/// `TransferContract` from `core/contract/balance_contract.proto`.
#[derive(Clone, PartialEq, Eq, ::prost::Message)]
pub struct TransferContract {
    #[prost(bytes = "vec", tag = "1")]
    pub owner_address: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub to_address: Vec<u8>,
    #[prost(int64, tag = "3")]
    pub amount: i64,
}

/// `CreateSmartContract` from `core/contract/smart_contract.proto`.
#[derive(Clone, PartialEq, Eq, ::prost::Message)]
pub struct CreateSmartContract {
    #[prost(bytes = "vec", tag = "1")]
    pub owner_address: Vec<u8>,
    #[prost(message, optional, tag = "2")]
    pub new_contract: Option<SmartContract>,
    #[prost(int64, tag = "3")]
    pub call_token_value: i64,
    #[prost(int64, tag = "4")]
    pub token_id: i64,
}

/// `SmartContract` from `core/contract/smart_contract.proto`. The `abi`
/// field (tag 3) is intentionally omitted: contracts are deployed without
/// an on-chain ABI (matching tronweb's `abi: []`).
#[derive(Clone, PartialEq, Eq, ::prost::Message)]
pub struct SmartContract {
    #[prost(bytes = "vec", tag = "1")]
    pub origin_address: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub contract_address: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    pub bytecode: Vec<u8>,
    #[prost(int64, tag = "5")]
    pub call_value: i64,
    #[prost(int64, tag = "6")]
    pub consume_user_resource_percent: i64,
    #[prost(string, tag = "7")]
    pub name: String,
    #[prost(int64, tag = "8")]
    pub origin_energy_limit: i64,
}

/// Contract type discriminants (`Transaction.Contract.ContractType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ContractType {
    TransferContract = 1,
    CreateSmartContract = 30,
    TriggerSmartContract = 31,
}

/// Returns the `type.googleapis.com` type URL used inside the `Any`
/// parameter wrapper for the given contract type.
pub const fn type_url(ct: ContractType) -> &'static str {
    match ct {
        ContractType::TransferContract => "type.googleapis.com/protocol.TransferContract",
        ContractType::CreateSmartContract => "type.googleapis.com/protocol.CreateSmartContract",
        ContractType::TriggerSmartContract => "type.googleapis.com/protocol.TriggerSmartContract",
    }
}

/// Computes the transaction id: sha256 of the protobuf-encoded `raw_data`.
pub fn txid(raw: &TransactionRaw) -> B256 {
    B256::from_slice(&Sha256::digest(raw.encode_to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    fn fixture() -> (Vec<u8>, String) {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/mainnet_trigger_tx.json")).unwrap();
        let raw = hex::decode(json["raw_data_hex"].as_str().unwrap()).unwrap();
        let txid = json["txID"].as_str().unwrap().to_string();
        (raw, txid)
    }

    #[test]
    fn fixture_roundtrip_is_byte_identical() {
        let (bytes, _) = fixture();
        let raw = TransactionRaw::decode(bytes.as_slice()).unwrap();
        assert!(!raw.contract.is_empty());
        assert_eq!(raw.encode_to_vec(), bytes, "re-encode must be byte-identical");
    }

    #[test]
    fn txid_matches_mainnet() {
        let (bytes, expected) = fixture();
        let raw = TransactionRaw::decode(bytes.as_slice()).unwrap();
        assert_eq!(hex::encode(txid(&raw)), expected);
    }

    #[test]
    fn trigger_contract_any_roundtrip() {
        let trig = TriggerSmartContract {
            owner_address: vec![0x41; 21],
            contract_address: vec![0x41; 21],
            call_value: 5,
            data: vec![0xde, 0xad],
            call_token_value: 0,
            token_id: 0,
        };
        let any = prost_types::Any {
            type_url: type_url(ContractType::TriggerSmartContract).to_string(),
            value: trig.encode_to_vec(),
        };
        let back = TriggerSmartContract::decode(any.value.as_slice()).unwrap();
        assert_eq!(back, trig);
    }
}
