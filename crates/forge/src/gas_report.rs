//! Gas reports.

use crate::{
    constants::{CHEATCODE_ADDRESS, HARDHAT_CONSOLE_ADDRESS},
    traces::{CallTraceArena, CallTraceDecoder, CallTraceNode, DecodedCallData},
};
use alloy_primitives::map::HashSet;
use comfy_table::{
    Cell, CellAlignment, Color, Table, modifiers::UTF8_ROUND_CORNERS, presets::ASCII_MARKDOWN,
};
use foundry_common::{TestFunctionExt, calc, shell};
use foundry_config::TronConfig;
use foundry_evm::traces::CallKind;
use foundry_tron_provider::{TxOptions, estimate_call_bandwidth, estimate_create_bandwidth};

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, fmt::Display};

/// Represents the gas report for a set of contracts.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GasReport {
    /// Whether to report any contracts.
    report_any: bool,
    /// Contracts to generate the report for.
    report_for: HashSet<String>,
    /// Contracts to ignore when generating the report.
    ignore: HashSet<String>,
    /// Whether to include gas reports for tests.
    include_tests: bool,
    /// Tron bandwidth-estimation parameters. `Some` on the Tron run path, which relabels the
    /// report to energy and adds a bandwidth (bytes) column; `None` leaves the EVM report
    /// byte-identical.
    #[serde(skip)]
    tron: Option<TronReport>,
    /// All contracts that were analyzed grouped by their identifier
    /// ``test/Counter.t.sol:CounterTest
    pub contracts: BTreeMap<String, ContractInfo>,
}

/// Tron parameters, derived from `[tron]` config, needed to estimate the bandwidth a broadcast
/// transaction would consume. Present only when the run targets the Tron network.
#[derive(Clone, Copy, Debug)]
struct TronReport {
    /// Transaction-level `fee_limit`/`expiration` knobs (`fee_limit` drives the raw's varint
    /// width).
    opts: TxOptions,
    /// `SmartContract.origin_energy_limit` (tag 8), stamped into deploy transactions.
    origin_energy_limit: i64,
    /// `SmartContract.consume_user_resource_percent` (tag 6), stamped into deploy transactions.
    consume_user_resource_percent: i64,
}

impl GasReport {
    pub fn new(
        report_for: impl IntoIterator<Item = String>,
        ignore: impl IntoIterator<Item = String>,
        include_tests: bool,
    ) -> Self {
        let report_for = report_for.into_iter().collect::<HashSet<_>>();
        let ignore = ignore.into_iter().collect::<HashSet<_>>();
        let report_any = report_for.is_empty() || report_for.contains("*");
        Self { report_any, report_for, ignore, include_tests, ..Default::default() }
    }

    /// Enables Tron energy/bandwidth reporting using the `[tron]` config: the existing gas column
    /// is relabeled to energy (on Tron `trace.gas_used` is TVM energy) and a bandwidth (bytes)
    /// column is estimated per frame from the broadcast transaction size.
    #[must_use]
    pub const fn with_tron(mut self, tron: &TronConfig) -> Self {
        self.tron = Some(TronReport {
            opts: TxOptions {
                fee_limit: tron.fee_limit,
                expiration_ms: tron.expiration as i64 * 1000,
            },
            origin_energy_limit: tron.origin_energy_limit,
            consume_user_resource_percent: tron.user_fee_percentage,
        });
        self
    }

    /// Whether the given contract should be reported.
    #[instrument(level = "trace", skip(self), ret)]
    fn should_report(&self, contract_name: &str) -> bool {
        if self.ignore.contains(contract_name) {
            let contains_anyway = self.report_for.contains(contract_name);
            if contains_anyway {
                // If the user listed the contract in 'gas_reports' (the foundry.toml field) a
                // report for the contract is generated even if it's listed in the ignore
                // list. This is addressed this way because getting a report you don't expect is
                // preferable than not getting one you expect. A warning is printed to stderr
                // indicating the "double listing".
                let _ = sh_warn!(
                    "{contract_name} is listed in both 'gas_reports' and 'gas_reports_ignore'."
                );
            }
            return contains_anyway;
        }
        self.report_any || self.report_for.contains(contract_name)
    }

    /// Analyzes the given traces and generates a gas report.
    pub async fn analyze(
        &mut self,
        arenas: impl IntoIterator<Item = &CallTraceArena>,
        decoder: &CallTraceDecoder,
    ) {
        for node in arenas.into_iter().flat_map(|arena| arena.nodes()) {
            self.analyze_node(node, decoder).await;
        }
    }

    async fn analyze_node(&mut self, node: &CallTraceNode, decoder: &CallTraceDecoder) {
        let trace = &node.trace;

        if trace.address == CHEATCODE_ADDRESS || trace.address == HARDHAT_CONSOLE_ADDRESS {
            return;
        }

        let Some(name) = decoder.contracts.get(&node.trace.address) else { return };
        let contract_name = name.rsplit(':').next().unwrap_or(name);

        if !self.should_report(contract_name) {
            return;
        }
        let contract_info = self.contracts.entry(name.clone()).or_default();
        let is_create_call = trace.kind.is_any_create();

        // Record contract deployment size and, on Tron, the deploy transaction's bandwidth. Both
        // are taken here (before the top-level guard) since they describe the init code (=
        // trace.data, the `CreateSmartContract.bytecode`) rather than execution, mirroring
        // `size`. Deploys in tests attach no TRX, so `call_value` is 0.
        if is_create_call {
            trace!(contract_name, "adding create size info");
            contract_info.size = trace.data.len();
            if let Some(tron) = &self.tron {
                contract_info.deployment_bandwidth = Some(estimate_create_bandwidth(
                    trace.data.to_vec(),
                    contract_name,
                    0,
                    tron.origin_energy_limit,
                    tron.consume_user_resource_percent,
                    &tron.opts,
                ));
            }
        }

        // Only include top-level calls which account for calldata and base (21.000) cost.
        // Only include Calls and Creates as only these calls are isolated in inspector.
        if trace.depth > 1 && (trace.kind == CallKind::Call || is_create_call) {
            return;
        }

        let decoded = || decoder.decode_function(&node.trace);

        if is_create_call {
            trace!(contract_name, "adding create gas info");
            contract_info.gas = trace.gas_used;
        } else if let Some(DecodedCallData { signature, .. }) = decoded().await.call_data {
            let name = signature.split('(').next().unwrap();
            // ignore any test/setup functions
            if self.include_tests || !name.test_function_kind().is_known() {
                trace!(contract_name, signature, "adding gas info");
                let gas_info = contract_info
                    .functions
                    .entry(name.to_string())
                    .or_default()
                    .entry(signature.clone())
                    .or_default();
                gas_info.frames.push(trace.gas_used);
                // On Tron, estimate the call transaction's bandwidth from its calldata (=
                // trace.data, the `TriggerSmartContract.data`). Reuses the same
                // per-frame machinery as energy, so fuzzed/dynamic calldata yields
                // min/avg/median/max bandwidth. `call_value` is 0.
                if let Some(tron) = &self.tron {
                    gas_info.bandwidth_frames.push(estimate_call_bandwidth(
                        trace.data.to_vec(),
                        0,
                        &tron.opts,
                    ));
                }
            }
        }
    }

    /// Finalizes the gas report by calculating the min, max, mean, and median for each function.
    #[must_use]
    pub fn finalize(mut self) -> Self {
        trace!("finalizing gas report");
        for contract in self.contracts.values_mut() {
            for sigs in contract.functions.values_mut() {
                for func in sigs.values_mut() {
                    func.frames.sort_unstable();
                    func.min = func.frames.first().copied().unwrap_or_default();
                    func.max = func.frames.last().copied().unwrap_or_default();
                    func.mean = calc::mean(&func.frames);
                    func.median = calc::median_sorted(&func.frames);
                    func.calls = func.frames.len() as u64;

                    // Tron: same statistics over the per-frame bandwidth estimates.
                    if !func.bandwidth_frames.is_empty() {
                        func.bandwidth_frames.sort_unstable();
                        func.bandwidth = Some(BandwidthStats {
                            min: func.bandwidth_frames.first().copied().unwrap_or_default(),
                            max: func.bandwidth_frames.last().copied().unwrap_or_default(),
                            mean: calc::mean(&func.bandwidth_frames),
                            median: calc::median_sorted(&func.bandwidth_frames),
                        });
                    }
                }
            }
        }
        self
    }
}

impl Display for GasReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        if shell::is_json() {
            writeln!(f, "{}", self.format_json_output())?;
        } else {
            for (name, contract) in &self.contracts {
                if contract.functions.is_empty() {
                    trace!(name, "gas report contract without functions");
                    continue;
                }

                let table = self.format_table_output(contract, name);
                writeln!(f, "\n{table}")?;
            }
        }

        Ok(())
    }
}

impl GasReport {
    fn format_json_output(&self) -> String {
        serde_json::to_string(
            &self
                .contracts
                .iter()
                .filter_map(|(name, contract)| {
                    if contract.functions.is_empty() {
                        trace!(name, "gas report contract without functions");
                        return None;
                    }

                    let functions = contract
                        .functions
                        .values()
                        .flat_map(|sigs| {
                            sigs.iter().map(|(sig, gas_info)| {
                                let display_name = sig.replace(':', "");
                                (display_name, gas_info)
                            })
                        })
                        .collect::<BTreeMap<_, _>>();

                    // On Tron the "gas" values are energy; the extra "bandwidth" keys (deployment
                    // and per-function, via GasInfo's skipped-when-None field) are emitted only on
                    // the Tron path, so EVM `--gas-report --json` output stays byte-identical.
                    let mut deployment = json!({
                        "gas": contract.gas,
                        "size": contract.size,
                    });
                    if let Some(bandwidth) = contract.deployment_bandwidth {
                        deployment["bandwidth"] = json!(bandwidth);
                    }

                    Some(json!({
                        "contract": name,
                        "deployment": deployment,
                        "functions": functions,
                    }))
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn format_table_output(&self, contract: &ContractInfo, name: &str) -> Table {
        let mut table = Table::new();
        if shell::is_markdown() {
            table.load_preset(ASCII_MARKDOWN);
        } else {
            table.apply_modifier(UTF8_ROUND_CORNERS);
        }

        // Tron layout decision: keep the EVM table byte-identical and, on the Tron path, (1)
        // relabel "Deployment Cost" -> "Deployment Energy" (`trace.gas_used` is TVM energy
        // there) and (2) append a bandwidth (bytes) block — a "Deployment Bandwidth" cell
        // and four "Bandwidth {Min,Avg,Median,Max}" columns after the energy columns. Extra
        // columns (not a second row block) keep every function on one line and
        // machine-diffable in snapshots.
        let is_tron = self.tron.is_some();

        table.set_header(vec![Cell::new(format!("{name} Contract")).fg(Color::Magenta)]);

        let mut deployment_header = vec![
            Cell::new(if is_tron { "Deployment Energy" } else { "Deployment Cost" })
                .fg(Color::Cyan),
            Cell::new("Deployment Size").fg(Color::Cyan),
        ];
        let mut deployment_row = vec![
            Cell::new(contract.gas.to_string()).set_alignment(CellAlignment::Right),
            Cell::new(contract.size.to_string()).set_alignment(CellAlignment::Right),
        ];
        if is_tron {
            deployment_header.push(Cell::new("Deployment Bandwidth").fg(Color::Cyan));
            deployment_row.push(
                Cell::new(contract.deployment_bandwidth.unwrap_or_default().to_string())
                    .set_alignment(CellAlignment::Right),
            );
        }
        table.add_row(deployment_header);
        table.add_row(deployment_row);

        // Add a blank row to separate deployment info from function info.
        table.add_row(vec![Cell::new("")]);

        let mut function_header = vec![
            Cell::new("Function Name"),
            Cell::new("Min").fg(Color::Green),
            Cell::new("Avg").fg(Color::Yellow),
            Cell::new("Median").fg(Color::Yellow),
            Cell::new("Max").fg(Color::Red),
            Cell::new("# Calls").fg(Color::Cyan),
        ];
        if is_tron {
            function_header.extend([
                Cell::new("Bandwidth Min").fg(Color::Green),
                Cell::new("Bandwidth Avg").fg(Color::Yellow),
                Cell::new("Bandwidth Median").fg(Color::Yellow),
                Cell::new("Bandwidth Max").fg(Color::Red),
            ]);
        }
        table.add_row(function_header);

        for (fname, sigs) in &contract.functions {
            for (sig, gas_info) in sigs {
                // Show function signature if overloaded else display function name.
                let display_name =
                    if sigs.len() == 1 { fname.clone() } else { sig.replace(':', "") };

                let mut row = vec![
                    Cell::new(display_name),
                    Cell::new(gas_info.min.to_string())
                        .fg(Color::Green)
                        .set_alignment(CellAlignment::Right),
                    Cell::new(gas_info.mean.to_string())
                        .fg(Color::Yellow)
                        .set_alignment(CellAlignment::Right),
                    Cell::new(gas_info.median.to_string())
                        .fg(Color::Yellow)
                        .set_alignment(CellAlignment::Right),
                    Cell::new(gas_info.max.to_string())
                        .fg(Color::Red)
                        .set_alignment(CellAlignment::Right),
                    Cell::new(gas_info.calls.to_string()).set_alignment(CellAlignment::Right),
                ];
                if is_tron {
                    let bandwidth = gas_info.bandwidth.clone().unwrap_or_default();
                    row.extend([
                        Cell::new(bandwidth.min.to_string())
                            .fg(Color::Green)
                            .set_alignment(CellAlignment::Right),
                        Cell::new(bandwidth.mean.to_string())
                            .fg(Color::Yellow)
                            .set_alignment(CellAlignment::Right),
                        Cell::new(bandwidth.median.to_string())
                            .fg(Color::Yellow)
                            .set_alignment(CellAlignment::Right),
                        Cell::new(bandwidth.max.to_string())
                            .fg(Color::Red)
                            .set_alignment(CellAlignment::Right),
                    ]);
                }
                table.add_row(row);
            }
        }

        table
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContractInfo {
    pub gas: u64,
    pub size: usize,
    /// Estimated Tron deployment bandwidth in bytes. `Some` only on the Tron path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_bandwidth: Option<u64>,
    /// Function name -> Function signature -> GasInfo
    pub functions: BTreeMap<String, BTreeMap<String, GasInfo>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GasInfo {
    pub calls: u64,
    pub min: u64,
    pub mean: u64,
    pub median: u64,
    pub max: u64,

    #[serde(skip)]
    pub frames: Vec<u64>,

    /// Estimated Tron bandwidth (bytes) statistics. `Some` only on the Tron path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<BandwidthStats>,

    #[serde(skip)]
    pub bandwidth_frames: Vec<u64>,
}

/// Tron bandwidth (bytes) statistics for a function, mirroring the energy min/avg/median/max.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BandwidthStats {
    pub min: u64,
    pub mean: u64,
    pub median: u64,
    pub max: u64,
}
