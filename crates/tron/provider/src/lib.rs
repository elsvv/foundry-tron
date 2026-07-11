//! # foundry-tron-provider
//!
//! Async HTTP client for Tron nodes (TronGrid / java-tron `/wallet/*` API).

mod client;

pub use client::{NowBlock, TronError, TronProvider, TxInfo};
