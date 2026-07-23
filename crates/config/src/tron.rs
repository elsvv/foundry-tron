//! Tron network specific configuration types.

use serde::{Deserialize, Serialize};

/// Default `fee_limit` in SUN (1000 TRX). Caps the amount of TRX that may be burned for a single
/// transaction.
const DEFAULT_FEE_LIMIT: i64 = 1_000_000_000;

/// Default `origin_energy_limit` for deployed contracts.
const DEFAULT_ORIGIN_ENERGY_LIMIT: i64 = 10_000_000;

/// Default `consume_user_resource_percent` (percentage of energy paid by the caller).
const DEFAULT_USER_FEE_PERCENTAGE: i64 = 100;

/// Default transaction `expiration` in seconds.
const DEFAULT_EXPIRATION: u64 = 60;

/// Default for the TIP-491 dynamic-energy penalty model in `forge test --gas-report`.
const DEFAULT_DYNAMIC_ENERGY: bool = true;

/// Configuration for the Tron network, mirroring the `[tron]` section of `foundry.toml`.
///
/// These values map onto protobuf fields of the Tron transaction (`fee_limit` on
/// `Transaction.raw_data`; `origin_energy_limit` and `consume_user_resource_percent` on
/// `SmartContract`). This crate keeps the values raw and does not depend on the Tron provider;
/// conversion into transaction options is performed by the consumer (`cast`/`forge script`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TronConfig {
    /// Maximum TRX, in SUN, that may be burned for a single transaction (`raw_data.fee_limit`).
    ///
    /// 1 TRX = 1_000_000 SUN.
    #[serde(default = "default_fee_limit")]
    pub fee_limit: i64,

    /// Energy limit contributed by the contract owner on deployment (`SmartContract`, tag 8).
    #[serde(default = "default_origin_energy_limit")]
    pub origin_energy_limit: i64,

    /// Percentage (0-100) of energy paid by the caller instead of the contract owner
    /// (`SmartContract.consume_user_resource_percent`, tag 6).
    #[serde(default = "default_user_fee_percentage")]
    pub user_fee_percentage: i64,

    /// Transaction expiration window in seconds, added to the current time when building a
    /// transaction. Conversion to milliseconds is performed by the consumer.
    #[serde(default = "default_expiration")]
    pub expiration: u64,

    /// Whether `forge test --gas-report` models the TIP-491 dynamic-energy penalty on a Tron
    /// fork. When on (the default), the report fetches each contract's live energy factor from
    /// the fork node and adds a penalty column; when off, or on a non-fork run, the report stays
    /// base-energy only. Has no effect off the Tron network.
    #[serde(default = "default_dynamic_energy")]
    pub dynamic_energy: bool,
}

impl Default for TronConfig {
    fn default() -> Self {
        Self {
            fee_limit: DEFAULT_FEE_LIMIT,
            origin_energy_limit: DEFAULT_ORIGIN_ENERGY_LIMIT,
            user_fee_percentage: DEFAULT_USER_FEE_PERCENTAGE,
            expiration: DEFAULT_EXPIRATION,
            dynamic_energy: DEFAULT_DYNAMIC_ENERGY,
        }
    }
}

const fn default_fee_limit() -> i64 {
    DEFAULT_FEE_LIMIT
}

const fn default_origin_energy_limit() -> i64 {
    DEFAULT_ORIGIN_ENERGY_LIMIT
}

const fn default_user_fee_percentage() -> i64 {
    DEFAULT_USER_FEE_PERCENTAGE
}

const fn default_expiration() -> u64 {
    DEFAULT_EXPIRATION
}

const fn default_dynamic_energy() -> bool {
    DEFAULT_DYNAMIC_ENERGY
}
