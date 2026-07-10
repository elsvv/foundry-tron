//! # foundry-tron-primitives
//!
//! Tron protocol primitives: address codec (base58check / 0x41-hex),
//! protobuf transactions (txID = sha256(raw_data)), secp256k1 signing.

pub mod address;
pub mod keys;
pub mod proto;
pub mod units;
