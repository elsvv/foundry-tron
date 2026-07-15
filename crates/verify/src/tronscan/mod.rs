//! TronScan contract-verification provider.
//!
//! TronScan exposes a **keyless**, **synchronous** `multipart/form-data` verify
//! endpoint (`POST /api/solidity/contract/verify`). Unlike Etherscan there is no
//! async submission GUID: the POST returns the terminal result directly
//! (`data.status`: `2006` success, `2001` already verified, `2007`/`2008` fail).
//! Confirmation is a re-query of `POST /api/solidity/contract/info`
//! (`data.status == 2` means verified).
//!
//! The public API has no official documentation; every field and status code
//! here is cross-checked against the TronScan frontend, two third-party verifier
//! implementations (`across-protocol`, `mapprotocol`), and live probes of an
//! already-verified mainnet contract (`TMv7hAfswe2EvXG4nUeNFGEgNWE8Joedtu`,
//! which stores `compiler="tron_v0.8.25+commit.77bd169f"`).
//!
//! Source is submitted as a single flattened Solidity file. The vanilla-solc dry
//! run and the IPFS `bytecodeHash` assertion used by the Etherscan flattened path
//! are intentionally skipped: the Tron project already compiles through the
//! native `tron-solc`, whose bytecode does not run on vanilla solc.

use crate::{
    provider::{VerificationContext, VerificationProvider, VerificationProviderType},
    verify::{VerifyArgs, VerifyCheckArgs},
};
use alloy_json_abi::Function;
use alloy_primitives::hex;
use eyre::{Context, Result, eyre};
use foundry_cli::utils::read_constructor_args_file;
use foundry_common::{abi::encode_function_args, flatten, retry::RetryError};
use foundry_config::Config;
use foundry_tron_primitives::to_base58;
use foundry_tron_solc::tronscan_compiler_string;
use reqwest::multipart;
use semver::Version;
use serde::Deserialize;
use std::time::Duration;

/// Tron mainnet chain id (`728126428`).
pub(crate) const TRON_MAINNET_CHAIN_ID: u64 = 728_126_428;
/// Tron Nile testnet chain id (`3448148188`).
pub(crate) const TRON_NILE_CHAIN_ID: u64 = 3_448_148_188;

/// Canonical mainnet TronScan API host.
const MAINNET_API_HOST: &str = "https://apilist.tronscanapi.com";
/// Nile testnet TronScan API host (NOT `nile.tronscan.org`, which is the frontend).
const NILE_API_HOST: &str = "https://nileapi.tronscan.org";
/// Mainnet TronScan contract-page base (browser link).
const MAINNET_BROWSER: &str = "https://tronscan.org/#/contract";
/// Nile TronScan contract-page base (browser link).
const NILE_BROWSER: &str = "https://nile.tronscan.org/#/contract";

/// The `data.status` returned by `/api/solidity/contract/info` for a verified contract.
const INFO_STATUS_VERIFIED: i64 = 2;

/// The resolved TronScan hosts for a verification run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TronscanHosts {
    /// API base without a trailing slash, e.g. `https://nileapi.tronscan.org`.
    pub api: String,
    /// Browser contract-page base, e.g. `https://nile.tronscan.org/#/contract`.
    /// `None` when routed via `--verifier-url` to an unknown host.
    pub browser: Option<&'static str>,
}

/// Resolves the TronScan API/browser hosts from the chain id and an optional
/// `--verifier-url` override.
///
/// A `--verifier-url` always wins for the API host (so private TronScan mirrors
/// work); the browser link is still filled in when the chain id is recognized.
/// When neither a URL nor a recognized chain id is available the caller cannot
/// know where to submit, so this returns a clear error asking for `--verifier-url`.
pub(crate) fn resolve_tronscan_hosts(
    chain_id: Option<u64>,
    verifier_url: Option<&str>,
) -> Result<TronscanHosts> {
    let browser = match chain_id {
        Some(TRON_MAINNET_CHAIN_ID) => Some(MAINNET_BROWSER),
        Some(TRON_NILE_CHAIN_ID) => Some(NILE_BROWSER),
        _ => None,
    };

    if let Some(url) = verifier_url {
        return Ok(TronscanHosts { api: url.trim_end_matches('/').to_string(), browser });
    }

    match chain_id {
        Some(TRON_MAINNET_CHAIN_ID) => {
            Ok(TronscanHosts { api: MAINNET_API_HOST.to_string(), browser })
        }
        Some(TRON_NILE_CHAIN_ID) => Ok(TronscanHosts { api: NILE_API_HOST.to_string(), browser }),
        Some(other) => eyre::bail!(
            "cannot route TronScan verification for chain id {other}: only Tron mainnet \
             ({TRON_MAINNET_CHAIN_ID}) and Nile ({TRON_NILE_CHAIN_ID}) have known TronScan API \
             hosts. Pass `--verifier-url <https://host>` to target another TronScan instance."
        ),
        None => eyre::bail!(
            "cannot determine the Tron chain id for TronScan verification. Set `chain_id` in \
             foundry.toml ({TRON_MAINNET_CHAIN_ID} for mainnet, {TRON_NILE_CHAIN_ID} for Nile) or \
             pass `--verifier-url <https://host>`."
        ),
    }
}

/// Verdict derived from a TronScan verify response, independent of the network
/// layer so it can be unit-tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerifyVerdict {
    /// `data.status == 2006`.
    Success,
    /// `data.status == 2001`.
    AlreadyVerified,
    /// `code != 200`, or a failing/unknown `data.status` (e.g. `2007`/`2008`).
    Failed,
}

/// Maps the TronScan verify response `code`/`data.status` onto a [`VerifyVerdict`].
pub(crate) const fn verify_verdict(code: i64, status: Option<i64>) -> VerifyVerdict {
    if code != 200 {
        return VerifyVerdict::Failed;
    }
    match status {
        Some(2006) => VerifyVerdict::Success,
        Some(2001) => VerifyVerdict::AlreadyVerified,
        _ => VerifyVerdict::Failed,
    }
}

/// The TronScan verify endpoint response envelope.
#[derive(Debug, Deserialize)]
struct VerifyResponse {
    code: i64,
    #[serde(default)]
    errmsg: Option<String>,
    #[serde(default)]
    data: Option<VerifyData>,
}

#[derive(Debug, Deserialize)]
struct VerifyData {
    #[serde(default)]
    status: Option<i64>,
    #[serde(default)]
    message: Option<String>,
}

/// The `/api/solidity/contract/info` response envelope (only the fields we need).
#[derive(Debug, Deserialize)]
struct InfoResponse {
    #[serde(default)]
    data: Option<InfoData>,
}

#[derive(Debug, Deserialize)]
struct InfoData {
    #[serde(default)]
    status: Option<i64>,
}

/// The prepared, network-independent verify form: text fields plus the flattened
/// source uploaded as the `files` part. Held separately so it can be rebuilt for
/// each retry attempt (a `multipart::Form` is consumed when sent).
struct TronscanForm {
    fields: Vec<(&'static str, String)>,
    source: String,
    filename: String,
}

/// TronScan verification provider.
#[derive(Clone, Debug)]
pub struct TronscanVerificationProvider {
    hosts: TronscanHosts,
}

impl TronscanVerificationProvider {
    /// Creates a provider targeting the given resolved hosts.
    pub(crate) const fn new(hosts: TronscanHosts) -> Self {
        Self { hosts }
    }

    /// Builds a fresh HTTP client with a generous timeout for the source upload.
    fn http_client() -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .wrap_err("failed to build TronScan HTTP client")
    }

    /// The browser contract-page URL, or the base58 address alone when the host is unknown.
    fn contract_url(&self, address_b58: &str) -> String {
        match self.hosts.browser {
            Some(base) => format!("{base}/{address_b58}"),
            None => address_b58.to_string(),
        }
    }

    /// Assembles the text fields and flattened source for the verify form.
    fn build_form(&self, args: &VerifyArgs, context: &VerificationContext) -> Result<TronscanForm> {
        let compiler = tronscan_compiler_string(&context.compiler_version).ok_or_else(|| {
            eyre!(
                "no pinned TronScan compiler string for tron-solc {}. Supported: 0.8.25, 0.8.26, \
                 0.8.27. Pass `--compiler-version <x.y.z>` matching a pinned tron-solc release.",
                context.compiler_version
            )
        })?;

        // Flatten with the Tron project (native tron-solc). No vanilla-solc dry run and no IPFS
        // bytecodeHash assertion: those are Etherscan-specific and wrong for the Tron toolchain.
        let source = flatten(context.project.clone(), &context.target_path)?;

        let constructor_params = self.constructor_params(args, context)?;
        let fields = assemble_fields(
            args,
            &context.config,
            &context.target_name,
            &context.compiler_version,
            compiler,
            constructor_params,
        );

        Ok(TronscanForm { fields, source, filename: format!("{}.sol", context.target_name) })
    }

    /// Returns the ABI-encoded constructor arguments as hex (no `0x`), or `None`
    /// when the contract takes none.
    fn constructor_params(
        &self,
        args: &VerifyArgs,
        context: &VerificationContext,
    ) -> Result<Option<String>> {
        if args.guess_constructor_args {
            eyre::bail!(
                "--guess-constructor-args is not supported for TronScan verification; pass \
                 `--constructor-args <hex>` or `--constructor-args-path <file>` instead."
            );
        }

        if let Some(path) = &args.constructor_args_path {
            let abi = context.get_target_abi()?;
            let constructor = abi
                .constructor()
                .ok_or_else(|| eyre!("can't retrieve constructor info from artifact ABI"))?;
            let func = Function {
                name: "constructor".to_string(),
                inputs: constructor.inputs.clone(),
                outputs: vec![],
                state_mutability: alloy_json_abi::StateMutability::NonPayable,
            };
            let encoded = encode_function_args(&func, read_constructor_args_file(path.clone())?)?;
            let encoded = hex::encode(encoded);
            // Strip the 4-byte (8 hex char) function selector; keep only the encoded args.
            return Ok(Some(strip_constructor_selector(&encoded).to_string()));
        }

        Ok(args.constructor_args.as_deref().map(|s| strip_hex_prefix(s).to_string()))
    }

    /// Queries `/api/solidity/contract/info` and returns the stored `data.status`
    /// (`2` == verified), or `None` when the contract is unknown / not yet stored.
    async fn query_info(&self, address_b58: &str) -> Result<Option<i64>> {
        let url = format!("{}/api/solidity/contract/info", self.hosts.api);
        let resp = Self::http_client()?
            .post(&url)
            .json(&serde_json::json!({ "contractAddress": address_b58 }))
            .send()
            .await
            .wrap_err("TronScan /info request failed")?;
        let body = resp.text().await.wrap_err("reading TronScan /info response failed")?;
        let parsed: InfoResponse = serde_json::from_str(&body)
            .wrap_err_with(|| format!("could not parse TronScan /info response: {body}"))?;
        Ok(parsed.data.and_then(|d| d.status))
    }

    /// Best-effort pre-check: `true` when `/info` already reports the contract as
    /// verified. Network failures are swallowed (returns `false`) so a flaky
    /// pre-check never blocks a real submission.
    async fn is_already_verified(&self, address_b58: &str) -> bool {
        matches!(self.query_info(address_b58).await, Ok(Some(INFO_STATUS_VERIFIED)))
    }
}

/// Renders a boolean as TronScan's `"1"`/`"0"` string.
fn bool_flag(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

/// Assembles the ordered TronScan verify text fields from the resolved verify
/// args, config, compiler string, and pre-resolved constructor params.
///
/// Pure and network-free: it derives the optimizer/runs/evmVersion/viaIR/license
/// values and lays out every field name and value shape — base58 `contractAddress`
/// and `0x`-free `constructorParams` — without touching the project or the wire, so
/// the exact form contract is unit-testable offline (the flattened source and the
/// pinned compiler string are resolved by the caller).
fn assemble_fields(
    args: &VerifyArgs,
    config: &Config,
    target_name: &str,
    compiler_version: &Version,
    compiler: String,
    constructor_params: Option<String>,
) -> Vec<(&'static str, String)> {
    let optimizer_on = args.num_of_optimizations.is_some() || config.optimizer == Some(true);
    let runs = args.num_of_optimizations.or(config.optimizer_runs).unwrap_or(200);
    // Send the EVM version tron-solc actually compiled with, not the raw config value: solc clamps
    // a too-new `evm_version` (e.g. the harness default `osaka`) down to the highest fork the
    // compiler supports (`prague` for 0.8.27, `cancun` for 0.8.25/0.8.26). TronScan rejects a
    // version the compiler never used, so mirror `get_solc_standard_json_input`'s
    // `normalize_evm_version`.
    let evm_version = config
        .evm_version
        .normalize_version_solc(compiler_version)
        .unwrap_or(config.evm_version)
        .to_string();
    let via_ir = args.via_ir || config.via_ir;
    // TronScan's `license` is the numeric Etherscan license code (already parsed by
    // `license_type`). Default to `1` (No License) when the user did not supply one.
    let license = args.license_type.clone().unwrap_or_else(|| "1".to_string());

    let mut fields: Vec<(&'static str, String)> = vec![
        ("contractAddress", to_base58(args.address)),
        ("contractName", target_name.to_string()),
        ("compiler", compiler),
        ("optimizer", bool_flag(optimizer_on)),
        ("runs", runs.to_string()),
        ("license", license),
        ("evmVersion", evm_version),
        ("viaIR", bool_flag(via_ir)),
    ];

    // `constructorParams` is ABI-encoded constructor args as hex WITHOUT the `0x` prefix (the
    // frontend field id is `constructorParams`; the across README's `constructorArguments` is a
    // documented slip). Skip the field entirely when the contract takes no constructor args.
    if let Some(params) = constructor_params
        && !params.is_empty()
    {
        fields.push(("constructorParams", params));
    }

    fields
}

/// Strips the 4-byte (8 hex-char) function selector that `encode_function_args`
/// prepends, leaving just the ABI-encoded constructor arguments TronScan expects
/// for `constructorParams`.
fn strip_constructor_selector(encoded_hex: &str) -> &str {
    &encoded_hex[8..]
}

/// Normalizes user-supplied `--constructor-args` hex to TronScan's no-`0x` shape.
fn strip_hex_prefix(value: &str) -> &str {
    value.trim_start_matches("0x")
}

/// Builds a fresh multipart form from the prepared fields (a `multipart::Form` is
/// consumed on send, so each retry rebuilds it).
fn build_multipart(form: &TronscanForm) -> Result<multipart::Form> {
    let mut mp = multipart::Form::new();
    for (name, value) in &form.fields {
        mp = mp.text(*name, value.clone());
    }
    let part = multipart::Part::text(form.source.clone())
        .file_name(form.filename.clone())
        .mime_str("text/plain")
        .wrap_err("failed to build TronScan source upload part")?;
    Ok(mp.part("files", part))
}

#[async_trait::async_trait]
impl VerificationProvider for TronscanVerificationProvider {
    fn provider_type(&self) -> VerificationProviderType {
        VerificationProviderType::Tronscan
    }

    async fn preflight_verify_check(
        &mut self,
        args: VerifyArgs,
        context: VerificationContext,
    ) -> Result<()> {
        // Validate that the form can be assembled (pinned compiler string, flattenable source,
        // resolvable constructor args) without sending anything.
        let _ = self.build_form(&args, &context)?;
        Ok(())
    }

    async fn submit(
        &mut self,
        args: VerifyArgs,
        context: VerificationContext,
    ) -> Result<Option<VerifyCheckArgs>> {
        let address_b58 = to_base58(args.address);

        if !args.skip_is_verified_check && self.is_already_verified(&address_b58).await {
            sh_status!(
                "Contract [{}] {} is already verified on TronScan. Skipping verification.",
                context.target_name,
                address_b58
            )?;
            return Ok(None);
        }

        let form = self.build_form(&args, &context)?;
        let client = Self::http_client()?;
        let url = format!("{}/api/solidity/contract/verify", self.hosts.api);

        let verdict =
            args.retry
                .into_retry()
                .run_async_until_break(|| async {
                    sh_status!(
                        "Submitting TronScan verification for [{}] {}.",
                        context.target_name,
                        address_b58
                    )
                    .map_err(RetryError::Break)?;

                    let multipart = build_multipart(&form).map_err(RetryError::Break)?;
                    let resp =
                        client.post(&url).multipart(multipart).send().await.map_err(|e| {
                            RetryError::Retry(eyre!("TronScan request failed: {e}"))
                        })?;
                    let status = resp.status();
                    let body = resp.text().await.map_err(|e| {
                        RetryError::Retry(eyre!("reading TronScan response failed: {e}"))
                    })?;

                    if !status.is_success() {
                        return Err(RetryError::Retry(eyre!(
                            "TronScan returned HTTP {status}: {body}"
                        )));
                    }

                    let parsed: VerifyResponse = serde_json::from_str(&body).map_err(|e| {
                        RetryError::Break(eyre!("could not parse TronScan response ({e}): {body}"))
                    })?;

                    trace!(?parsed, "TronScan verify response");

                    match verify_verdict(parsed.code, parsed.data.as_ref().and_then(|d| d.status)) {
                        verdict @ (VerifyVerdict::Success | VerifyVerdict::AlreadyVerified) => {
                            Ok(verdict)
                        }
                        VerifyVerdict::Failed => {
                            let detail = parsed
                                .data
                                .as_ref()
                                .and_then(|d| d.message.clone())
                                .or(parsed.errmsg.clone())
                                .unwrap_or_else(|| body.clone());
                            Err(RetryError::Break(eyre!(
                                "TronScan verification failed (code {}, status {:?}): {detail}",
                                parsed.code,
                                parsed.data.as_ref().and_then(|d| d.status),
                            )))
                        }
                    }
                })
                .await?;

        let contract_url = self.contract_url(&address_b58);
        match verdict {
            VerifyVerdict::Success => {
                sh_status!(
                    "Submitted contract for verification:\n\tContract: [{}] {}\n\tURL: {}",
                    context.target_name,
                    address_b58,
                    contract_url
                )?;
                if args.print_submission_result_to_stdout {
                    sh_println!("{address_b58}\t{contract_url}")?;
                }
                Ok(Some(VerifyCheckArgs {
                    id: address_b58,
                    etherscan: args.etherscan,
                    retry: args.retry,
                    verifier: args.verifier,
                }))
            }
            VerifyVerdict::AlreadyVerified => {
                sh_status!(
                    "Contract [{}] {} is already verified on TronScan.\n\tURL: {}",
                    context.target_name,
                    address_b58,
                    contract_url
                )?;
                Ok(None)
            }
            // `Failed` is turned into an error inside the retry closure above.
            VerifyVerdict::Failed => unreachable!("failed verdict is surfaced as an error"),
        }
    }

    async fn check(&self, args: VerifyCheckArgs) -> Result<()> {
        // `id` carries the base58 contract address (set by `submit`).
        let address_b58 = args.id.clone();
        args.retry
            .into_retry()
            .run_async_until_break(|| async {
                match self.query_info(&address_b58).await {
                    Ok(Some(INFO_STATUS_VERIFIED)) => {
                        let _ = sh_status!("Contract successfully verified on TronScan");
                        Ok(())
                    }
                    Ok(other) => Err(RetryError::Retry(eyre!(
                        "TronScan verification still pending (info status {other:?})..."
                    ))),
                    Err(err) => {
                        Err(RetryError::Retry(eyre!("failed to query TronScan /info: {err}")))
                    }
                }
            })
            .await
            .wrap_err("checking TronScan verification result failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_json_abi::{Param, StateMutability};
    use alloy_primitives::{Address, address};
    use clap::Parser;
    use foundry_compilers::artifacts::EvmVersion;

    /// A known (hex20, base58) Tron address vector (mainnet USDT) so the
    /// `contractAddress` field is asserted against a real base58check encoding
    /// rather than round-tripping `to_base58` against itself.
    const USDT_HEX20: Address = address!("a614f803b6fd780986a42c78ec9c7f77e6ded13c");
    const USDT_BASE58: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";

    /// Parses a minimal `VerifyArgs` carrying only the target address; every other
    /// field defaults so tests can set exactly the inputs they exercise.
    fn verify_args() -> VerifyArgs {
        VerifyArgs::try_parse_from([
            "verify-contract",
            "0xa614f803b6fd780986a42c78ec9c7f77e6ded13c",
        ])
        .expect("VerifyArgs should parse from a bare address")
    }

    #[test]
    fn form_fields_full_shape_and_base58_address() {
        let args = verify_args();
        let config = Config {
            optimizer: Some(true),
            optimizer_runs: Some(500),
            evm_version: EvmVersion::London,
            via_ir: false,
            ..Default::default()
        };

        let fields = assemble_fields(
            &args,
            &config,
            "MyContract",
            &Version::new(0, 8, 25),
            "tron_v0.8.25+commit.77bd169f".to_string(),
            None,
        );

        // Every field name/value shape, in order. `contractAddress` is the base58check
        // encoding of the 0x41-prefixed address; no `constructorParams` when there are none.
        assert_eq!(
            fields,
            vec![
                ("contractAddress", USDT_BASE58.to_string()),
                ("contractName", "MyContract".to_string()),
                ("compiler", "tron_v0.8.25+commit.77bd169f".to_string()),
                ("optimizer", "1".to_string()),
                ("runs", "500".to_string()),
                ("license", "1".to_string()),
                ("evmVersion", "london".to_string()),
                ("viaIR", "0".to_string()),
            ]
        );
        // Sanity: the asserted base58 really is `to_base58` of the hex20 vector.
        assert_eq!(to_base58(USDT_HEX20), USDT_BASE58);
    }

    /// Looks up a field's value in the assembled form, or `None` when absent.
    fn field<'a>(fields: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
        fields.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn form_fields_optimizer_and_runs_derivation() {
        let compiler = || "tron_v0.8.27+commit.a0c62e50".to_string();
        let ver = Version::new(0, 8, 27);

        // Optimizer off and no explicit runs: `"0"` with the solc default of 200 runs.
        let args = verify_args();
        let config = Config::default();
        let fields = assemble_fields(&args, &config, "C", &ver, compiler(), None);
        assert_eq!(field(&fields, "optimizer"), Some("0"));
        assert_eq!(field(&fields, "runs"), Some("200"));

        // Optimizer enabled via config with a configured run count.
        let config =
            Config { optimizer: Some(true), optimizer_runs: Some(1337), ..Default::default() };
        let fields = assemble_fields(&args, &config, "C", &ver, compiler(), None);
        assert_eq!(field(&fields, "optimizer"), Some("1"));
        assert_eq!(field(&fields, "runs"), Some("1337"));

        // `--optimizer-runs` (args.num_of_optimizations) forces the optimizer on and wins over
        // the config run count.
        let mut args = verify_args();
        args.num_of_optimizations = Some(999);
        let config =
            Config { optimizer: Some(false), optimizer_runs: Some(1337), ..Default::default() };
        let fields = assemble_fields(&args, &config, "C", &ver, compiler(), None);
        assert_eq!(field(&fields, "optimizer"), Some("1"));
        assert_eq!(field(&fields, "runs"), Some("999"));
    }

    #[test]
    fn form_fields_evm_version_normalizes_to_compiler() {
        let compiler = || "tron_v0.8.27+commit.a0c62e50".to_string();
        let args = verify_args();

        let normalized = |evm: EvmVersion, ver: Version| {
            let config = Config { evm_version: evm, ..Default::default() };
            let fields = assemble_fields(&args, &config, "C", &ver, compiler(), None);
            field(&fields, "evmVersion").unwrap().to_string()
        };

        // A too-new config `evm_version` is clamped down to what the compiler actually supports:
        // 0.8.27 caps at prague, 0.8.25/0.8.26 cap at cancun (see upstream normalize_version_solc).
        assert_eq!(normalized(EvmVersion::Osaka, Version::new(0, 8, 27)), "prague");
        assert_eq!(normalized(EvmVersion::Osaka, Version::new(0, 8, 26)), "cancun");
        assert_eq!(normalized(EvmVersion::Osaka, Version::new(0, 8, 25)), "cancun");
        // An in-range version passes through unchanged.
        assert_eq!(normalized(EvmVersion::London, Version::new(0, 8, 27)), "london");
    }

    #[test]
    fn form_fields_license_and_via_ir() {
        let compiler = || "tron_v0.8.27+commit.a0c62e50".to_string();
        let ver = Version::new(0, 8, 27);

        // No `--license`: default to Etherscan license code `1` (No License). viaIR off.
        let args = verify_args();
        let config = Config::default();
        let fields = assemble_fields(&args, &config, "C", &ver, compiler(), None);
        assert_eq!(field(&fields, "license"), Some("1"));
        assert_eq!(field(&fields, "viaIR"), Some("0"));

        // Explicit license code and `--via-ir` on the args.
        let mut args = verify_args();
        args.license_type = Some("3".to_string());
        args.via_ir = true;
        let fields = assemble_fields(&args, &config, "C", &ver, compiler(), None);
        assert_eq!(field(&fields, "license"), Some("3"));
        assert_eq!(field(&fields, "viaIR"), Some("1"));

        // viaIR is also enabled when only the config sets it.
        let args = verify_args();
        let config = Config { via_ir: true, ..Default::default() };
        let fields = assemble_fields(&args, &config, "C", &ver, compiler(), None);
        assert_eq!(field(&fields, "viaIR"), Some("1"));
    }

    #[test]
    fn form_fields_constructor_params_appended_or_skipped() {
        let compiler = || "tron_v0.8.27+commit.a0c62e50".to_string();
        let ver = Version::new(0, 8, 27);
        let args = verify_args();
        let config = Config::default();

        // Present, non-empty: appended verbatim as the trailing `constructorParams` field (the
        // value carries no `0x`, exactly as `constructor_params` produced it).
        let fields =
            assemble_fields(&args, &config, "C", &ver, compiler(), Some("deadbeef".to_string()));
        assert_eq!(fields.last(), Some(&("constructorParams", "deadbeef".to_string())));

        // Empty string: no field (a contract with no args must not send an empty param).
        let fields = assemble_fields(&args, &config, "C", &ver, compiler(), Some(String::new()));
        assert_eq!(field(&fields, "constructorParams"), None);

        // None: no field.
        let fields = assemble_fields(&args, &config, "C", &ver, compiler(), None);
        assert_eq!(field(&fields, "constructorParams"), None);
    }

    #[test]
    fn constructor_params_transforms_strip_selector_and_prefix() {
        // The path branch ABI-encodes the args (selector-prefixed) and must drop the 4-byte
        // selector so only the encoded arguments remain. Encode a real `uint256` constructor and
        // confirm the 8 leading hex chars are removed, leaving the 32-byte word for 42 (0x2a).
        let func = Function {
            name: "constructor".to_string(),
            inputs: vec![Param::parse("uint256 x").unwrap()],
            outputs: vec![],
            state_mutability: StateMutability::NonPayable,
        };
        let encoded = hex::encode(encode_function_args(&func, ["42"]).unwrap());
        assert_eq!(encoded.len(), 8 + 64, "selector (4 bytes) + one 32-byte word");
        let params = strip_constructor_selector(&encoded);
        assert_eq!(params, &encoded[8..]);
        assert_eq!(params, "000000000000000000000000000000000000000000000000000000000000002a");

        // The literal `--constructor-args` branch strips a leading `0x` and leaves bare hex intact.
        assert_eq!(strip_hex_prefix("0xdeadbeef"), "deadbeef");
        assert_eq!(strip_hex_prefix("deadbeef"), "deadbeef");
    }

    #[test]
    fn routes_mainnet_and_nile_by_chain_id() {
        let mainnet = resolve_tronscan_hosts(Some(TRON_MAINNET_CHAIN_ID), None).unwrap();
        assert_eq!(mainnet.api, "https://apilist.tronscanapi.com");
        assert_eq!(mainnet.browser, Some("https://tronscan.org/#/contract"));

        let nile = resolve_tronscan_hosts(Some(TRON_NILE_CHAIN_ID), None).unwrap();
        assert_eq!(nile.api, "https://nileapi.tronscan.org");
        assert_eq!(nile.browser, Some("https://nile.tronscan.org/#/contract"));
    }

    #[test]
    fn verifier_url_overrides_host_and_keeps_known_browser() {
        // Override wins for the API host; the browser link is still filled from the chain id.
        let hosts =
            resolve_tronscan_hosts(Some(TRON_NILE_CHAIN_ID), Some("https://mirror.example/api/"))
                .unwrap();
        assert_eq!(hosts.api, "https://mirror.example/api");
        assert_eq!(hosts.browser, Some("https://nile.tronscan.org/#/contract"));

        // Unknown chain id + URL override: API set, browser unknown.
        let hosts = resolve_tronscan_hosts(Some(11111), Some("https://mirror.example")).unwrap();
        assert_eq!(hosts.api, "https://mirror.example");
        assert_eq!(hosts.browser, None);
    }

    #[test]
    fn undeterminable_host_is_a_clear_error() {
        let err = resolve_tronscan_hosts(None, None).unwrap_err().to_string();
        assert!(err.contains("--verifier-url"), "message: {err}");
        assert!(err.contains("chain_id"), "message: {err}");

        let err = resolve_tronscan_hosts(Some(999), None).unwrap_err().to_string();
        assert!(err.contains("999"), "message: {err}");
        assert!(err.contains("--verifier-url"), "message: {err}");
    }

    #[test]
    fn verdict_maps_status_codes() {
        assert_eq!(verify_verdict(200, Some(2006)), VerifyVerdict::Success);
        assert_eq!(verify_verdict(200, Some(2001)), VerifyVerdict::AlreadyVerified);
        assert_eq!(verify_verdict(200, Some(2007)), VerifyVerdict::Failed);
        assert_eq!(verify_verdict(200, Some(2008)), VerifyVerdict::Failed);
        assert_eq!(verify_verdict(200, None), VerifyVerdict::Failed);
        // A non-200 envelope is always a failure regardless of any status.
        assert_eq!(verify_verdict(-1, Some(2006)), VerifyVerdict::Failed);
    }

    #[test]
    fn contract_url_uses_browser_base_when_known() {
        let provider = TronscanVerificationProvider::new(TronscanHosts {
            api: "https://nileapi.tronscan.org".to_string(),
            browser: Some("https://nile.tronscan.org/#/contract"),
        });
        assert_eq!(
            provider.contract_url("TX7izXWcmofRYonzdcThrS78jifMtVWCuf"),
            "https://nile.tronscan.org/#/contract/TX7izXWcmofRYonzdcThrS78jifMtVWCuf"
        );

        let provider = TronscanVerificationProvider::new(TronscanHosts {
            api: "https://mirror.example".to_string(),
            browser: None,
        });
        assert_eq!(
            provider.contract_url("TX7izXWcmofRYonzdcThrS78jifMtVWCuf"),
            "TX7izXWcmofRYonzdcThrS78jifMtVWCuf"
        );
    }

    /// Live, read-only re-query of a known verified mainnet contract's `/info`, asserting the exact
    /// `compiler` string format and `status == 2` (verified) that our pins and `query_info` depend
    /// on. Read-only: no verification is submitted and no TRX is spent. Gated on `TRON_LIVE=1`;
    /// `eprintln!` for the skip notice is the sanctioned gated-test pattern, so allow the
    /// workspace's disallowed-macro lint. Not a fork test, so the `fork` naming rule does not
    /// apply.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::disallowed_macros)]
    async fn live_tronscan_info_compiler_string() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!(
                "skipped live_tronscan_info_compiler_string: set TRON_LIVE=1 to run the read-only \
                 TronScan /info probe"
            );
            return;
        }

        // across AcrossEventEmitter on Tron mainnet: verified, built with tron-solc 0.8.25.
        const VERIFIED_MAINNET: &str = "TMv7hAfswe2EvXG4nUeNFGEgNWE8Joedtu";

        let provider = TronscanVerificationProvider::new(TronscanHosts {
            api: MAINNET_API_HOST.to_string(),
            browser: Some(MAINNET_BROWSER),
        });

        // Our narrow `query_info` must report this contract as verified.
        assert_eq!(
            provider.query_info(VERIFIED_MAINNET).await.unwrap(),
            Some(INFO_STATUS_VERIFIED),
            "known verified contract must report /info status == 2"
        );

        // Assert the full API contract our compiler-string pins target: `tron_v<longVersion>`.
        let url = format!("{MAINNET_API_HOST}/api/solidity/contract/info");
        let body: serde_json::Value = reqwest::Client::new()
            .post(&url)
            .json(&serde_json::json!({ "contractAddress": VERIFIED_MAINNET }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let data = &body["data"];
        assert_eq!(data["status"].as_i64(), Some(INFO_STATUS_VERIFIED));
        assert_eq!(
            data["compiler"].as_str(),
            Some("tron_v0.8.25+commit.77bd169f"),
            "TronScan compiler string format must match our tron_v<longVersion> pins"
        );
        // The value our pins produce for 0.8.25 must equal the live-stored compiler string.
        assert_eq!(
            tronscan_compiler_string(&semver::Version::new(0, 8, 25)).as_deref(),
            data["compiler"].as_str(),
        );
    }
}
