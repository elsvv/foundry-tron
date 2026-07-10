//! # foundry-tron-primitives
//!
//! Tron protocol primitives: address codec (base58check / 0x41-hex),
//! protobuf transactions (txID = sha256(raw_data)), secp256k1 signing.

pub mod address;
pub mod keys;
pub mod proto;
pub mod sign;
pub mod tapos;
pub mod units;

pub use address::{parse as parse_address, to_base58, to_hex41};
pub use proto::{ContractType, Transaction, TransactionRaw, TriggerSmartContract, txid};
pub use sign::{SignedTronTx, sign_raw};
pub use tapos::ref_block;
