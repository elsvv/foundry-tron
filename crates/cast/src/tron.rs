//! Shared helpers for the Tron (`network = "tron"`) code paths in `cast`.
//!
//! Tron transactions are protobuf, signed over `sha256(raw_data)` and broadcast
//! through the HTTP `/wallet/*` API rather than `eth_sendRawTransaction`, so
//! `cast send`/`call`/`balance` branch into these helpers before touching any
//! alloy provider. Routing every Tron I/O through here keeps address parsing,
//! provider construction and option conversion consistent across commands.

use alloy_ens::NameOrAddress;
use alloy_json_abi::Function;
use alloy_primitives::{Address, B256, U256, hex};
use eyre::{Result, WrapErr};
use foundry_common::{
    abi::{encode_function_args, encode_function_args_raw, get_func},
    shell,
};
use foundry_config::{Config, TronConfig};
use foundry_tron_primitives::{
    address::parse as parse_tron, to_base58, to_hex41, units::format_sun_as_trx,
};
use foundry_tron_provider::{TronProvider, TxInfo, TxOptions};
use std::{str::FromStr, time::Duration};

/// Environment variable carrying the TronGrid API key, forwarded as the
/// `TRON-PRO-API-KEY` header (spec §4.7).
const TRON_API_KEY_ENV: &str = "TRON_PRO_API_KEY";

/// Fixed poll interval, in seconds, used while waiting for confirmation.
const POLL_INTERVAL_SECS: u64 = 3;

/// Builds a [`TronProvider`] from the RPC endpoint resolved in `config`.
///
/// The endpoint comes from `--rpc-url` (which resolves the builtin `tron` /
/// `nile` / `shasta` aliases) or `eth_rpc_url` in `foundry.toml`. A `TRON_PRO_API_KEY`
/// environment variable, when present, is attached as the `TRON-PRO-API-KEY` header.
pub fn tron_provider(config: &Config) -> Result<TronProvider> {
    let url = config
        .get_rpc_url()
        .ok_or_else(|| {
            eyre::eyre!(
                "a Tron RPC endpoint is required; pass --rpc-url (e.g. nile) or set one in foundry.toml"
            )
        })?
        .wrap_err("failed to resolve the Tron RPC endpoint")?
        .into_owned();
    let mut provider = TronProvider::new(&url)?;
    if let Ok(key) = std::env::var(TRON_API_KEY_ENV)
        && !key.is_empty()
    {
        provider = provider.with_api_key(key);
    }
    Ok(provider)
}

/// Parses a Tron address in base58check (`T…`), `41…`-hex or `0x…` form.
pub fn parse_tron_address(input: &str) -> Result<Address> {
    parse_tron(input.trim()).wrap_err_with(|| format!("invalid Tron address: {input}"))
}

/// clap value parser for address CLI arguments (`cast send`/`call`/`balance`
/// destinations) that also accepts Tron forms.
///
/// [`NameOrAddress::from_str`] only recognises `0x…` addresses and dotted ENS
/// names, rejecting base58check (`T…`) and `41…`-hex Tron addresses before the
/// command runs. This parser keeps that behaviour, then falls back to parsing a
/// Tron address and storing its 20-byte form as [`NameOrAddress::Address`], so
/// Tron destinations survive argument parsing. Non-address, non-Tron strings
/// still error.
pub fn parse_name_or_tron_address(s: &str) -> std::result::Result<NameOrAddress, String> {
    if let Ok(noa) = NameOrAddress::from_str(s) {
        return Ok(noa);
    }
    match parse_tron(s) {
        Ok(addr) => Ok(NameOrAddress::Address(addr)),
        Err(e) => Err(format!("'{s}' is not a 0x/ENS address and not a valid Tron address ({e})")),
    }
}

/// Extracts the string form of a resolved [`NameOrAddress`] for Tron re-parsing:
/// an `Address` becomes its `0x…` form, a `Name` is returned verbatim (ENS names
/// are then rejected downstream, as they are unsupported on Tron).
pub fn name_or_address_str(who: &NameOrAddress) -> String {
    match who {
        NameOrAddress::Address(a) => a.to_string(),
        NameOrAddress::Name(n) => n.clone(),
    }
}

/// Renders the three interchangeable forms of a Tron address (base58check,
/// 0x41-hex and 0x). Offline; backs `cast tron-address`.
pub fn format_tron_address(input: &str) -> Result<String> {
    let addr = parse_tron_address(input)?;
    Ok(format!(
        "Base58:  {}\nHex(41): {}\n0x:      {}",
        to_base58(addr),
        to_hex41(addr),
        hex::encode_prefixed(addr),
    ))
}

/// Converts a [`TronConfig`] into transaction [`TxOptions`]: `fee_limit` stays in
/// SUN, and `expiration` is converted from seconds to milliseconds.
pub const fn tx_options(cfg: &TronConfig) -> TxOptions {
    TxOptions { fee_limit: cfg.fee_limit, expiration_ms: (cfg.expiration as i64) * 1000 }
}

/// Derives `(attempts, interval)` polling parameters that cover the transaction's
/// expiration window plus a 30s margin at a fixed 3s interval, so a poll never
/// gives up before a still-valid transaction can be mined.
pub fn poll_params(cfg: &TronConfig) -> (u32, Duration) {
    let attempts = (cfg.expiration + 30).div_ceil(POLL_INTERVAL_SECS).max(1) as u32;
    (attempts, Duration::from_secs(POLL_INTERVAL_SECS))
}

/// Encodes contract calldata for a Tron trigger call from an optional signature
/// (or raw hex calldata) and its arguments.
///
/// - `None` signature yields empty calldata (a value-only trigger).
/// - Raw hex in `sig` is decoded as-is (mirrors `cast`'s `--data` path).
/// - Otherwise `sig` is a Solidity function signature; address-typed arguments given in Tron form
///   (`T…`/`41…`) are rewritten to `0x…` before ABI encoding.
///
/// Returns the calldata and, when a function signature was given, the parsed
/// [`Function`] so callers can decode return data.
pub fn encode_calldata(sig: Option<&str>, args: &[String]) -> Result<(Vec<u8>, Option<Function>)> {
    let Some(sig) = sig else {
        return Ok((Vec::new(), None));
    };
    // Raw hex calldata (e.g. from `--data`) is used verbatim.
    if let Ok(data) = hex::decode(sig) {
        return Ok((data, None));
    }
    let func = get_func(sig)?;
    let args = rewrite_tron_address_args(&func, args);
    Ok((encode_function_args(&func, &args)?, Some(func)))
}

/// Encodes constructor arguments (no selector) for a Tron `CreateSmartContract`
/// deploy. Returns empty bytes when no constructor signature is given.
pub fn encode_constructor_args(sig: Option<&str>, args: &[String]) -> Result<Vec<u8>> {
    let Some(sig) = sig else {
        return Ok(Vec::new());
    };
    let func = get_func(sig)?;
    let args = rewrite_tron_address_args(&func, args);
    encode_function_args_raw(&func, &args)
}

/// Interprets a `--value` (denominated in SUN on Tron) as an `i64` amount.
pub fn value_sun(value: Option<U256>) -> Result<i64> {
    match value {
        None => Ok(0),
        Some(v) => {
            let as_u64 = u64::try_from(v).map_err(|_| eyre::eyre!("--value exceeds u64 SUN"))?;
            i64::try_from(as_u64).map_err(|_| eyre::eyre!("--value exceeds i64 SUN"))
        }
    }
}

/// Cross-checks the locally derived deploy address against the node's reported
/// `contract_address`; a mismatch, or a missing address on a failed deploy, is a
/// hard error (spec §4.5, plan fact §1).
pub fn verify_deploy_address(local: Address, info: &TxInfo) -> Result<()> {
    match info.contract_address {
        Some(node) if node == local => Ok(()),
        Some(node) => eyre::bail!(
            "deploy address mismatch: local {} != node {}",
            to_hex41(local),
            to_hex41(node)
        ),
        None => eyre::bail!(
            "node reported no contract_address for the deploy (local {}); the deploy may have failed",
            to_hex41(local)
        ),
    }
}

/// A Tron energy/bandwidth/fee estimate for a contract call — the result
/// `cast estimate` prints on Tron.
#[derive(Debug, Clone, Copy)]
pub struct TronEstimate {
    /// Total energy the call would consume (TIP-491 penalty included).
    pub energy_used: u64,
    /// TIP-491 penalty portion of `energy_used` (0 when taken from
    /// `estimateenergy`, which reports no breakdown).
    pub energy_penalty: u64,
    /// On-chain bandwidth, in bytes, the signed transaction would occupy.
    pub bandwidth_bytes: u64,
    /// Suggested `fee_limit` in SUN (energy cost + buffer, clamped to
    /// `getMaxFeeLimit`).
    pub suggested_fee_limit_sun: u64,
    /// Estimated burned cost in SUN: `energy × energy_fee + bandwidth × transaction_fee`.
    pub est_cost_sun: u64,
}

/// Prints a Tron call estimate. The estimate is the command's primary result, so
/// it goes to stdout: a labeled table in text mode, the same fields as an object
/// under `--json`.
pub fn print_tron_estimate(est: &TronEstimate) -> Result<()> {
    if shell::is_json() {
        let obj = serde_json::json!({
            "energy_used": est.energy_used,
            "energy_penalty": est.energy_penalty,
            "bandwidth_bytes": est.bandwidth_bytes,
            "suggested_fee_limit_sun": est.suggested_fee_limit_sun,
            "est_cost_trx": format_sun_as_trx(est.est_cost_sun),
        });
        sh_println!("{}", serde_json::to_string_pretty(&obj)?)?;
    } else {
        sh_println!("energy used:         {}", est.energy_used)?;
        sh_println!("energy penalty:      {}", est.energy_penalty)?;
        sh_println!("bandwidth (bytes):   {}", est.bandwidth_bytes)?;
        sh_println!("suggested fee_limit: {} SUN", est.suggested_fee_limit_sun)?;
        sh_println!("est. cost:           {} TRX", format_sun_as_trx(est.est_cost_sun))?;
    }
    Ok(())
}

/// Prints the extended TIP-491 energy/resource breakdown of a confirmed receipt
/// to stderr (status text). Total energy is annotated with its penalty portion
/// and base only when a penalty was charged; the caller/origin energy split and
/// bandwidth usage print only when non-zero, so an ordinary transfer stays terse
/// and existing penalty-free output is unchanged.
pub fn print_tron_resource_usage(info: &TxInfo) -> Result<()> {
    if info.energy_used > 0 {
        if info.energy_penalty_total > 0 {
            sh_status!(
                "energy used: {} (penalty {}, base {})",
                info.energy_used,
                info.energy_penalty_total,
                info.energy_used.saturating_sub(info.energy_penalty_total),
            )?;
        } else {
            sh_status!("energy used: {}", info.energy_used)?;
        }
        if info.energy_usage_caller > 0 || info.origin_energy_usage > 0 {
            sh_status!(
                "  caller/origin: {} / {}",
                info.energy_usage_caller,
                info.origin_energy_usage
            )?;
        }
    }
    if info.net_usage > 0 {
        sh_status!("bandwidth:   {} bytes", info.net_usage)?;
    }
    Ok(())
}

/// Prints a confirmed Tron transaction: status, energy and fee prose on stderr,
/// the txID (machine-readable primary result) on stdout.
pub fn print_tron_tx(txid: B256, info: &TxInfo) -> Result<()> {
    sh_status!("status:      {}", if info.success { "success" } else { "failed" })?;
    print_tron_resource_usage(info)?;
    sh_status!("fee:         {} TRX", format_sun_as_trx(info.fee_sun))?;
    sh_status!("block:       {}", info.block_number)?;
    sh_println!("{}", hex::encode(txid))?;
    Ok(())
}

/// Prints a confirmed Tron deploy: txID, status, energy and fee on stderr, and
/// the deployed contract address (base58, the primary machine-readable result)
/// on stdout. The 0x41-hex form goes to stderr for tooling that wants it.
pub fn print_tron_deploy(txid: B256, addr: Address, info: &TxInfo) -> Result<()> {
    sh_status!("txID:        {}", hex::encode(txid))?;
    sh_status!("status:      {}", if info.success { "success" } else { "failed" })?;
    print_tron_resource_usage(info)?;
    sh_status!("fee:         {} TRX", format_sun_as_trx(info.fee_sun))?;
    sh_status!("deployed to: {} ({})", to_base58(addr), to_hex41(addr))?;
    sh_println!("{}", to_base58(addr))?;
    Ok(())
}

/// Prints an unconfirmed (`--async`) Tron deploy: the locally derived address on
/// stdout and the txID on stderr. The address is computed from the txID via the
/// java-tron formula, so it is known before confirmation.
pub fn print_tron_deploy_async(txid: B256, addr: Address) -> Result<()> {
    sh_status!("txID:        {}", hex::encode(txid))?;
    sh_status!("deployed to: {} ({}) [unconfirmed]", to_base58(addr), to_hex41(addr))?;
    sh_println!("{}", to_base58(addr))?;
    Ok(())
}

/// Rewrites `address`-typed arguments supplied in Tron form (`T…`/`41…`) into
/// their `0x…` equivalent so alloy's ABI encoder accepts them. Non-address
/// arguments and already-`0x` addresses pass through unchanged.
fn rewrite_tron_address_args(func: &Function, args: &[String]) -> Vec<String> {
    func.inputs
        .iter()
        .map(|p| p.ty.as_str())
        .chain(std::iter::repeat(""))
        .zip(args)
        .map(|(ty, arg)| {
            if ty == "address"
                && !arg.starts_with("0x")
                && let Ok(addr) = parse_tron(arg.trim())
            {
                hex::encode_prefixed(addr)
            } else {
                arg.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_address_forms() {
        let base58 = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
        let hex41 = "41a614f803b6fd780986a42c78ec9c7f77e6ded13c";
        let hex0x = "0xa614f803b6fd780986a42c78ec9c7f77e6ded13c";
        let a = parse_tron_address(base58).unwrap();
        assert_eq!(parse_tron_address(hex41).unwrap(), a);
        assert_eq!(parse_tron_address(hex0x).unwrap(), a);
    }

    #[test]
    fn formats_three_forms() {
        let out = format_tron_address("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").unwrap();
        assert_eq!(
            out,
            "Base58:  TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t\n\
             Hex(41): 41a614f803b6fd780986a42c78ec9c7f77e6ded13c\n\
             0x:      0xa614f803b6fd780986a42c78ec9c7f77e6ded13c"
        );
    }

    #[test]
    fn rejects_bad_address() {
        assert!(format_tron_address("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6u").is_err());
    }

    #[test]
    fn value_parser_accepts_tron_and_eth_forms() {
        let target = parse_tron_address("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").unwrap();
        // base58check, 41-hex and 0x all resolve to the same 20-byte address.
        for s in [
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
            "41a614f803b6fd780986a42c78ec9c7f77e6ded13c",
            "0xa614f803b6fd780986a42c78ec9c7f77e6ded13c",
        ] {
            match parse_name_or_tron_address(s).unwrap() {
                NameOrAddress::Address(a) => assert_eq!(a, target),
                NameOrAddress::Name(n) => panic!("expected address, got name {n}"),
            }
        }
        // A dotted ENS name is still preserved as a name.
        assert!(matches!(
            parse_name_or_tron_address("vitalik.eth").unwrap(),
            NameOrAddress::Name(_)
        ));
        // Garbage is rejected.
        assert!(parse_name_or_tron_address("not-an-address").is_err());
    }

    #[test]
    fn poll_params_cover_expiration_plus_margin() {
        // 60s expiration + 30s margin over a 3s interval => 30 attempts (>= 90s).
        let cfg = TronConfig { expiration: 60, ..TronConfig::default() };
        let (attempts, interval) = poll_params(&cfg);
        assert_eq!(interval, Duration::from_secs(3));
        assert_eq!(attempts, 30);
        assert!((attempts as u64) * interval.as_secs() >= cfg.expiration + 30);
    }

    #[test]
    fn tx_options_convert_expiration_to_ms() {
        let cfg = TronConfig { fee_limit: 400_000_000, expiration: 30, ..TronConfig::default() };
        let opts = tx_options(&cfg);
        assert_eq!(opts.fee_limit, 400_000_000);
        assert_eq!(opts.expiration_ms, 30_000);
    }

    #[test]
    fn encodes_setnumber_calldata() {
        let (data, func) = encode_calldata(Some("setNumber(uint256)"), &["7".to_string()]).unwrap();
        assert_eq!(hex::encode(&data[..4]), "3fb5c1cb");
        assert_eq!(func.unwrap().name, "setNumber");
        // 4-byte selector + 32-byte word.
        assert_eq!(data.len(), 36);
    }

    #[test]
    fn empty_sig_is_empty_calldata() {
        let (data, func) = encode_calldata(None, &[]).unwrap();
        assert!(data.is_empty());
        assert!(func.is_none());
    }

    #[test]
    fn rewrites_tron_address_argument() {
        // A `T…` address argument is accepted and encoded as its 20-byte form.
        let (data, _) = encode_calldata(
            Some("transfer(address,uint256)"),
            &["TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t".to_string(), "1".to_string()],
        )
        .unwrap();
        // The address occupies the low 20 bytes of the first 32-byte word after the selector.
        assert_eq!(hex::encode(&data[4 + 12..4 + 32]), "a614f803b6fd780986a42c78ec9c7f77e6ded13c");
    }

    #[test]
    fn constructor_args_have_no_selector() {
        let data =
            encode_constructor_args(Some("constructor(uint256)"), &["42".to_string()]).unwrap();
        // Just the 32-byte encoded argument, no 4-byte selector.
        assert_eq!(data.len(), 32);
        assert_eq!(hex::encode(&data[31..]), "2a");
    }
}
