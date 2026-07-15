//! Tron network CLI tests.
//!
//! The live fork tests here run `forge test` with `network = "tron"` against the java-tron
//! `/jsonrpc` endpoint. That endpoint serves everything the fork backend needs (balance, code,
//! storage, blocks, chain id) but answers `eth_getTransactionCount` with a permanent `-32601`
//! stub, so a read-only fork only works with the Tron nonce shim installed on the fork provider
//! (see `TronNonceShimLayer` in `foundry_common::provider`). These tests are the end-to-end proof
//! that the shim, the Tron network dispatch, and the Tron energy model all cooperate on a real
//! mainnet fork.

use foundry_evm_networks::NetworkConfigs;
use foundry_tron_solc::{binary_path, default_version};

/// True when the pinned native `tron-solc` binary is cached on this machine
/// (`~/.foundry-tron/solc/tron-solc-<version>`), so an offline compile-and-run Tron test can build
/// without a network or a download. Used to gate compile-based Tron tests that must skip cleanly on
/// a CI box without the sha-pinned binary (rather than weaken the assertions).
fn tron_solc_cached() -> bool {
    binary_path(&default_version()).map(|p| p.exists()).unwrap_or(false)
}

/// Mainnet TronGrid `/jsonrpc` endpoint. It is the only reachable public java-tron `/jsonrpc`
/// (nileex.io does not mount `/jsonrpc`), so the live fork channel is mainnet-only. All reads
/// below are read-only; no transaction is broadcast and no TRX is spent.
const TRON_MAINNET_JSONRPC: &str = "https://api.trongrid.io/jsonrpc";

// Live, read-only mainnet fork test over the java-tron `/jsonrpc` endpoint.
//
// Forks Tron mainnet and reads the real USDT TRC-20 contract
// (`TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t`, 0x41-stripped hex
// `0xa614f803b6fd780986a42c78ec9c7f77e6ded13c`): asserts the stable contract constants
// (`name() == "Tether USD"`, `symbol() == "USDT"`, `decimals() == 6`), the live-but-nonzero
// aggregates (`totalSupply() > 0`, `balanceOf(rich) > 0`), that the contract bytecode is present
// (`extcodesize > 0`), and reads a concrete storage slot (the balances mapping at base slot 0)
// cross-checked against the ABI `balanceOf` result on the same block.
//
// It exercises both fork-construction paths through the shimmed provider: the `--fork-url`
// command-line path (`EvmOpts::get_fork`) and the `vm.createSelectFork` cheatcode path
// (`MultiFork::create_fork`).
//
// Gated on `TRON_LIVE=1` (read-only mainnet; no TRX spent). The `fork` token in the name
// satisfies the repo rule that forking tests are named with `fork`. `eprintln!` for the skip
// notice is the sanctioned gated-test pattern, so allow the workspace's disallowed-macro lint.
forgetest_init!(
    #[expect(clippy::disallowed_macros)]
    tron_mainnet_fork_reads_usdt,
    |prj, cmd| {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!(
                "skipped tron_mainnet_fork_reads_usdt: set TRON_LIVE=1 to run the live mainnet fork test"
            );
            return;
        }

        prj.update_config(|config| {
            config.networks = NetworkConfigs::with_tron();
            // The test template pins `solc = SOLC_VERSION` (currently 0.8.35), which the native
            // tron-solc resolver has no pinned build for. Clear it so the resolver falls back to
            // its pinned default (0.8.27), matching the sandbox project.
            config.solc = None;
        });

        prj.add_test(
            "TronUsdtFork.t.sol",
            r#"
// SPDX-License-Identifier: MIT OR Apache-2.0
// Own pragma so the harness does not inject `=SOLC_VERSION` (0.8.35), for which no native
// tron-solc build is pinned; the caret range is satisfied by the resolved tron-solc 0.8.27.
pragma solidity ^0.8.0;

import "forge-std/Test.sol";

interface IERC20 {
    function name() external view returns (string memory);
    function symbol() external view returns (string memory);
    function decimals() external view returns (uint8);
    function totalSupply() external view returns (uint256);
    function balanceOf(address) external view returns (uint256);
}

contract TronUsdtForkTest is Test {
    // Tron mainnet USDT (TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t), 0x41 prefix stripped.
    IERC20 constant USDT = IERC20(0xa614f803B6FD780986A42c78Ec9c7f77e6DeD13C);
    // A large USDT holder on Tron mainnet (TWd4WrZ9wn84f5x1hZhL4DHvk738ns5jwb).
    address constant RICH = 0xe28B3CfD4E0e909077821478E9FCB86B84be786e;

    function _assertUsdt() internal {
        // Stable contract constants.
        assertEq(USDT.name(), "Tether USD");
        assertEq(USDT.symbol(), "USDT");
        assertEq(USDT.decimals(), 6);

        // Live aggregates: assert `> 0`, never an exact value.
        assertGt(USDT.totalSupply(), 0);
        uint256 richBal = USDT.balanceOf(RICH);
        assertGt(richBal, 0);

        // Real contract bytecode is present on the fork.
        uint256 size;
        address a = address(USDT);
        assembly {
            size := extcodesize(a)
        }
        assertGt(size, 0);

        // Concrete storage-slot read: the balances mapping is at base slot 0, so the raw slot
        // `keccak256(abi.encode(holder, 0))` must equal the ABI `balanceOf` result on the same
        // forked block. This cross-checks the storage read path against the call path.
        bytes32 slot = keccak256(abi.encode(RICH, uint256(0)));
        uint256 stored = uint256(vm.load(address(USDT), slot));
        assertEq(stored, richBal);
        assertGt(stored, 0);
    }

    // Fork is established from the `--fork-url` command line (`EvmOpts::get_fork`).
    function test_tron_mainnet_fork_usdt_global_reads() public {
        _assertUsdt();

        // Energy sanity: the `name()` staticcall is metered by the Tron energy model. The exact
        // number depends on the fork block, so assert it lands in a plausible Tron-scale window
        // and log it rather than pinning an exact value.
        uint256 g0 = gasleft();
        USDT.name();
        uint256 used = g0 - gasleft();
        emit log_named_uint("usdt.name() energy", used);
        assertGt(used, 0);
        assertLt(used, 10_000_000);
    }

    // Fork is established via the `vm.createSelectFork` cheatcode (`MultiFork::create_fork`).
    function test_tron_mainnet_fork_usdt_createselectfork_reads() public {
        vm.createSelectFork("https://api.trongrid.io/jsonrpc");
        _assertUsdt();
    }
}
"#,
        );

        // 1) `--fork-url` / `EvmOpts::get_fork` path.
        cmd.args([
            "test",
            "--mt",
            "test_tron_mainnet_fork_usdt_global_reads",
            "--fork-url",
            TRON_MAINNET_JSONRPC,
            "-vvv",
        ])
        .assert_success();

        // 2) `vm.createSelectFork` / `MultiFork::create_fork` path (no global fork on the command
        //    line).
        cmd.forge_fuse()
            .args(["test", "--mt", "test_tron_mainnet_fork_usdt_createselectfork_reads", "-vvv"])
            .assert_success();
    }
);

// Offline guard: `forge test --fork-url .../jsonrpc --fork-block-number <historical>` on Tron must
// bail with the tip-only explanation. Tron forks are tip-only (java-tron `/jsonrpc` serves state
// only at TAG `latest`), so an explicit historical block would silently mix a historical block env
// with tip-only state. The guard fires before compilation and before the fork is constructed, so
// this test needs neither the `tron-solc` compiler nor a live endpoint: no `.sol` file is compiled
// and the URL is never contacted. The `fork` token in the name satisfies the forking-test naming
// rule.
forgetest_init!(tron_historical_fork_block_bails, |prj, cmd| {
    prj.update_config(|config| {
        config.networks = NetworkConfigs::with_tron();
        config.solc = None;
    });

    cmd.args([
        "test",
        "--fork-url",
        "https://api.trongrid.io/jsonrpc",
        "--fork-block-number",
        "12345678",
    ])
    .assert_failure()
    .stderr_eq(str![[r#"
...
Error: Tron forks are tip-only: state is served only at the chain tip (/jsonrpc serves state only at TAG latest). Drop --fork-block-number for a tip fork.

"#]]);
});

// Offline guard: `forge coverage --fork-url .../jsonrpc --fork-block-number <historical>` on Tron
// must bail with the same tip-only explanation as `forge test`. `forge coverage` builds and runs
// tests through its own path (not `compile_project`), so this exercises the fail-fast guard in
// `CoverageArgs::run` and the shared backstop in `TestArgs::run_tests`. Like the `forge test` case,
// the guard fires before compilation and before the fork is constructed, so no `.sol` file is
// compiled, the `tron-solc` compiler is never invoked, and the URL is never contacted. The `fork`
// token in the name satisfies the forking-test naming rule.
forgetest_init!(tron_historical_coverage_fork_block_bails, |prj, cmd| {
    prj.update_config(|config| {
        config.networks = NetworkConfigs::with_tron();
        config.solc = None;
    });

    cmd.args([
        "coverage",
        "--fork-url",
        "https://api.trongrid.io/jsonrpc",
        "--fork-block-number",
        "12345678",
    ])
    .assert_failure()
    .stderr_eq(str![[r#"
...
Error: Tron forks are tip-only: state is served only at the chain tip (/jsonrpc serves state only at TAG latest). Drop --fork-block-number for a tip fork.

"#]]);
});

// Cheatcode guard E2E: `vm.createSelectFork(url, blockNumber)` with an explicit block on Tron must
// revert with the tip-only explanation, while the block-less `vm.createSelectFork(url)` stays
// legal. This proves the cheatcode dispatch wiring (that `createSelectFork_1` reads the Tron
// network flag and reaches the guard), which the crate-level `ensure_tron_tip_only_fork` unit test
// cannot cover. The guard fires before any network I/O, so no endpoint is contacted
// (`vm.createSelectFork` never reaches the fork backend). Gated on `TRON_LIVE=1` because compiling
// the test contract needs the native `tron-solc` compiler; the `fork` token in the name satisfies
// the forking-test naming rule.
forgetest_init!(
    #[expect(clippy::disallowed_macros)]
    tron_createselectfork_historical_block_fork_reverts,
    |prj, cmd| {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!(
                "skipped tron_createselectfork_historical_block_fork_reverts: set TRON_LIVE=1 to compile with tron-solc and run"
            );
            return;
        }

        prj.update_config(|config| {
            config.networks = NetworkConfigs::with_tron();
            config.solc = None;
        });

        prj.add_test(
            "TronForkGuard.t.sol",
            r#"
// SPDX-License-Identifier: MIT OR Apache-2.0
// Own pragma so the harness does not inject `=SOLC_VERSION`, for which no native tron-solc build is
// pinned; the caret range is satisfied by the resolved tron-solc 0.8.27.
pragma solidity ^0.8.0;

import "forge-std/Test.sol";

contract TronForkGuardTest is Test {
    // The cheatcode-variant message from `ensure_tron_tip_only_fork` in fork.rs, prefixed by the
    // cheatcode name (`vm.createSelectFork: `) that the cheatcode dispatcher prepends to errors.
    string constant EXPECTED =
        "vm.createSelectFork: Tron forks are tip-only: state is served only at the chain tip (/jsonrpc serves state only at TAG latest). Drop the block number argument to fork at the tip.";

    // Wrapper so the cheatcode revert unwinds to the try/catch in the test below.
    function forkAtHistoricalBlock() external {
        vm.createSelectFork("https://api.trongrid.io/jsonrpc", 12345678);
    }

    // An explicit historical block through the `createSelectFork_1` cheatcode variant must revert
    // with the tip-only `CheatcodeError(string)`.
    function test_tron_createselectfork_historical_block_reverts() public {
        try this.forkAtHistoricalBlock() {
            revert("expected createSelectFork with a historical block to bail on tron");
        } catch (bytes memory err) {
            assertEq(err, abi.encodeWithSignature("CheatcodeError(string)", EXPECTED));
        }
    }
}
"#,
        );

        cmd.args(["test", "--mt", "test_tron_createselectfork_historical_block_reverts", "-vvv"])
            .assert_success();
    }
);

// Offline gas report on the Tron path: `forge test --gas-report` relabels the report to energy
// (`trace.gas_used` is TVM energy on Tron) and adds a bandwidth (bytes) block estimated from each
// transaction's protobuf size (`serialized_size(signed tx) + 64`). This compiles the Counter with
// the native `tron-solc` and runs locally (no fork, no network), so it is gated on the cached
// compiler and skipped cleanly when it is absent. The deterministic (non-fuzz) test keeps energy
// and bandwidth reproducible; the bandwidth figures are chain-derived constants for the default
// `fee_limit` (increment() -> 280, setNumber(uint256) -> 314; deployment from the init code).
forgetest_init!(
    #[expect(clippy::disallowed_macros)]
    tron_gas_report_energy_and_bandwidth,
    |prj, cmd| {
        if !tron_solc_cached() {
            eprintln!(
                "skipped tron_gas_report_energy_and_bandwidth: native tron-solc is not cached at ~/.foundry-tron/solc; set up the pinned binary (or TRON_SOLC_DOWNLOAD=1) to run this offline compile test"
            );
            return;
        }

        prj.update_config(|config| {
            config.networks = NetworkConfigs::with_tron();
            // Clear the harness-pinned solc (0.8.35, no native tron build) so the resolver falls
            // back to its pinned default; the Counter's `^0.8.13` pragma is satisfied
            // by tron-solc 0.8.27.
            config.solc = None;
            config.gas_reports = vec!["*".to_string()];
            config.gas_reports_ignore = vec![];
        });

        // Own the Counter source so the test is self-contained (`forgetest_init!` sets up forge-std
        // but not the default Counter contracts). Own `^0.8.13` pragma so the harness does not
        // inject `=SOLC_VERSION` (0.8.35), which has no native tron-solc build.
        prj.add_source(
            "Counter.sol",
            r#"
// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

contract Counter {
    uint256 public number;

    function setNumber(uint256 newNumber) public {
        number = newNumber;
    }

    function increment() public {
        number++;
    }
}
"#,
        );

        // Deterministic, non-fuzz test: fixed calldata and storage transitions keep the energy and
        // bandwidth numbers reproducible for the snapshot.
        prj.add_test(
            "Counter.t.sol",
            r#"
// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "forge-std/Test.sol";
import {Counter} from "../src/Counter.sol";

contract CounterTest is Test {
    Counter public counter;

    function setUp() public {
        counter = new Counter();
    }

    function test_Increment() public {
        counter.increment();
        assertEq(counter.number(), 1);
    }

    function test_SetNumber() public {
        counter.setNumber(42);
        assertEq(counter.number(), 42);
    }
}
"#,
        );

        // Warm the build cache first so the `--gas-report` snapshots below stay focused on the
        // report (compilation is skipped and forge-std's compiler warnings do not appear in
        // stdout).
        cmd.forge_fuse().args(["build"]).assert_success();

        // Table: the deployment row is relabeled to "Deployment Energy" and gains a "Deployment
        // Bandwidth" cell (853 bytes for the 555-byte Counter init code), and each function gains a
        // "Bandwidth {Min,Avg,Median,Max}" block. Energy matches the plan-E golden (read `number()`
        // 414, write `setNumber`/`increment` ~20438). Deployment Energy is 0 because the local Tron
        // model does not meter create-frame energy (a pre-existing property of `trace.gas_used` for
        // creates, unrelated to this report); deployment bandwidth is still exact from the init
        // code.
        cmd.forge_fuse().args(["test", "--gas-report"]).assert_success().stdout_eq(str![[r#"
No files changed, compilation skipped

Ran 2 tests for test/Counter.t.sol:CounterTest
[PASS] test_Increment() ([GAS])
[PASS] test_SetNumber() ([GAS])
Suite result: ok. 2 passed; 0 failed; 0 skipped; [ELAPSED]

╭----------------------------------+-----------------+----------------------+--------+-------+---------+---------------+---------------+------------------+---------------╮
| src/Counter.sol:Counter Contract |                 |                      |        |       |         |               |               |                  |               |
+=========================================================================================================================================================================+
| Deployment Energy                | Deployment Size | Deployment Bandwidth |        |       |         |               |               |                  |               |
|----------------------------------+-----------------+----------------------+--------+-------+---------+---------------+---------------+------------------+---------------|
|                                0 |             555 |                  853 |        |       |         |               |               |                  |               |
|----------------------------------+-----------------+----------------------+--------+-------+---------+---------------+---------------+------------------+---------------|
|                                  |                 |                      |        |       |         |               |               |                  |               |
|----------------------------------+-----------------+----------------------+--------+-------+---------+---------------+---------------+------------------+---------------|
| Function Name                    | Min             | Avg                  | Median | Max   | # Calls | Bandwidth Min | Bandwidth Avg | Bandwidth Median | Bandwidth Max |
|----------------------------------+-----------------+----------------------+--------+-------+---------+---------------+---------------+------------------+---------------|
| increment                        |           20414 |                20414 |  20414 | 20414 |       1 |           280 |           280 |              280 |           280 |
|----------------------------------+-----------------+----------------------+--------+-------+---------+---------------+---------------+------------------+---------------|
| number                           |             414 |                  414 |    414 |   414 |       2 |           280 |           280 |              280 |           280 |
|----------------------------------+-----------------+----------------------+--------+-------+---------+---------------+---------------+------------------+---------------|
| setNumber                        |           20438 |                20438 |  20438 | 20438 |       1 |           314 |           314 |              314 |           314 |
╰----------------------------------+-----------------+----------------------+--------+-------+---------+---------------+---------------+------------------+---------------╯


Ran 1 test suite [ELAPSED]: 2 tests passed, 0 failed, 0 skipped (2 total tests)

"#]]);

        // JSON: `gas` (= energy on Tron) and `size` stay as before; the Tron-only `bandwidth` keys
        // (deployment scalar + per-function stats) are the only additions, gated by
        // `skip_serializing_if` so EVM `--gas-report --json` output is unaffected.
        cmd.forge_fuse().args(["test", "--gas-report", "--json"]).assert_success().stdout_eq(
            str![[r#"
[{"contract":"src/Counter.sol:Counter","deployment":{"gas":0,"size":555,"bandwidth":853},"functions":{"increment()":{"calls":1,"min":20414,"mean":20414,"median":20414,"max":20414,"bandwidth":{"min":280,"mean":280,"median":280,"max":280}},"number()":{"calls":2,"min":414,"mean":414,"median":414,"max":414,"bandwidth":{"min":280,"mean":280,"median":280,"max":280}},"setNumber(uint256)":{"calls":1,"min":20438,"mean":20438,"median":20438,"max":20438,"bandwidth":{"min":314,"mean":314,"median":314,"max":314}}}}]


"#]],
        );
    }
);

// Offline: `forge create --verify` on Tron no longer bails at argument validation. Before plan H
// (task H4) the Tron path rejected `--verify` up front ("--verify is not supported on tron yet").
// It now assembles a `VerifyArgs` and routes the deployed contract to the TronScan provider, with a
// pre-broadcast preflight (pinned tron-solc compiler string, routable TronScan host, flattenable
// source) that mirrors the generic `forge create` path. This test drives the dry run (no
// `--broadcast`, so no network and no TRX): the preflight must pass offline and the command must
// reach the dry-run output instead of the removed bail. Compiling and flattening the Counter needs
// the native `tron-solc`, so it is gated on the cached compiler and skipped cleanly when absent.
forgetest_init!(
    #[expect(clippy::disallowed_macros)]
    tron_create_verify_preflight_no_longer_bails,
    |prj, cmd| {
        if !tron_solc_cached() {
            eprintln!(
                "skipped tron_create_verify_preflight_no_longer_bails: native tron-solc is not cached at ~/.foundry-tron/solc; set up the pinned binary (or TRON_SOLC_DOWNLOAD=1) to run this offline compile test"
            );
            return;
        }

        prj.update_config(|config| {
            config.networks = NetworkConfigs::with_tron();
            // Nile chain id routes the verify preflight to nileapi.tronscan.org (mirrors the
            // sandbox's `chain_id = 3448148188`). Without it the preflight would (correctly) demand
            // `--verifier-url`; setting it exercises the real host-routing path offline.
            config.chain = Some(3_448_148_188u64.into());
            // Clear the harness-pinned solc (0.8.35, no native tron build) so the resolver falls
            // back to its pinned default (0.8.27); the Counter's `^0.8.13` pragma is satisfied.
            config.solc = None;
        });

        // Own the Counter source with its own `^0.8.13` pragma so the harness does not inject
        // `=SOLC_VERSION` (0.8.35), which has no native tron-solc build.
        prj.add_source(
            "Counter.sol",
            r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

contract Counter {
    uint256 public number;

    function setNumber(uint256 newNumber) public {
        number = newNumber;
    }

    function increment() public {
        number++;
    }
}
"#,
        );

        // Dry run (no `--broadcast`): compiles, runs the TronScan verify preflight, then prints the
        // dry-run output. No signer, RPC, or TRX involved.
        cmd.forge_fuse().args([
            "create",
            "src/Counter.sol:Counter",
            "--verify",
            "--license-type",
            "MIT",
        ]);
        let output = cmd.execute();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "forge create --verify dry run failed\nstdout: {stdout}\nstderr: {stderr}"
        );
        // The stage-1 rejection is gone: `--verify` is honored, not bailed at arg validation.
        assert!(
            !stderr.contains("--verify is not supported"),
            "forge create --verify must no longer bail on tron\nstderr: {stderr}"
        );
        // The preflight passed and the command reached the dry-run stop-before-network output.
        assert!(
            stderr.contains("Dry run enabled"),
            "expected the dry-run banner after a passing verify preflight\nstderr: {stderr}"
        );
    }
);

// Live acceptance gate for the TronScan verification path: deploy a fresh Counter on Nile with
// `forge create`, then verify it end-to-end through the new `forge verify-contract` TronScan
// provider. This is the ONLY test that spends TRX, so it is double-gated on `TRON_LIVE=1` AND
// `TRON_VERIFY_E2E=1` and needs the funded Nile key in `TRON_PRIVATE_KEY`; it is never run in cron.
// It proves the full round trip our unit tests cannot: multipart submit, the pinned
// `tron_v0.8.27+commit.19164bed` compiler string being accepted by TronScan, and the `--watch`
// `/info` re-query reaching `status == 2`. The 0.8.27 compiler string was live-confirmed accepted
// during implementation (Nile contract `TDKFWYmx4D4makUGMg6kuVvWCjuXnCTJHQ`). `eprintln!` for the
// skip notice is the sanctioned gated-test pattern, so allow the workspace's disallowed-macro lint.
forgetest_init!(
    #[expect(clippy::disallowed_macros)]
    live_tron_verify_contract_e2e_on_nile,
    |prj, cmd| {
        if std::env::var("TRON_LIVE").is_err() || std::env::var("TRON_VERIFY_E2E").is_err() {
            eprintln!(
                "skipped live_tron_verify_contract_e2e_on_nile: set TRON_LIVE=1 AND TRON_VERIFY_E2E=1 (this test spends TRX on Nile) to run the deploy+verify acceptance gate"
            );
            return;
        }
        let Ok(private_key) = std::env::var("TRON_PRIVATE_KEY") else {
            eprintln!(
                "skipped live_tron_verify_contract_e2e_on_nile: TRON_PRIVATE_KEY is not set (funded Nile key required to deploy)"
            );
            return;
        };

        prj.update_config(|config| {
            config.networks = NetworkConfigs::with_tron();
            // Nile chain id routes verification to nileapi.tronscan.org (mirrors the sandbox's
            // `chain_id = 3448148188`).
            config.chain = Some(3_448_148_188u64.into());
            // Clear the harness-pinned solc (0.8.35, no native tron build) so the resolver falls
            // back to its pinned default (0.8.27); the Counter's `^0.8.13` pragma is satisfied.
            config.solc = None;
        });

        // Own the Counter source with its own `^0.8.13` pragma so the harness does not inject
        // `=SOLC_VERSION` (0.8.35), which has no native tron-solc build.
        prj.add_source(
            "Counter.sol",
            r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

contract Counter {
    uint256 public number;

    function setNumber(uint256 newNumber) public {
        number = newNumber;
    }

    function increment() public {
        number++;
    }
}
"#,
        );

        // 1) Deploy a fresh Counter on Nile. `--json` gives us the deployed address to verify.
        cmd.forge_fuse().args([
            "create",
            "src/Counter.sol:Counter",
            "--rpc-url",
            "https://api.nileex.io",
            "--private-key",
            &private_key,
            "--broadcast",
            "--json",
        ]);
        let output = cmd.execute();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "deploy failed\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let deploy: serde_json::Value =
            serde_json::from_str(&stdout).expect("deploy did not emit JSON");
        // `verify-contract` accepts a Tron address in `41…`-hex (or base58 `T…`) form directly
        // (plan H, H4) — no manual 0x41 stripping. `deployedToHex` is exactly the `41…` form.
        let address = deploy["deployedToHex"].as_str().expect("deployedToHex missing");

        // 2) Verify the freshly deployed contract through the TronScan provider; `--watch` polls
        //    `/info` until `status == 2`. Success (exit 0 + the confirmation line) is the gate.
        cmd.forge_fuse().args([
            "verify-contract",
            address,
            "src/Counter.sol:Counter",
            "--license-type",
            "MIT",
            "--watch",
        ]);
        let output = cmd.execute();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "verify failed\nstdout: {}\nstderr: {stderr}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            stderr.contains("Contract successfully verified on TronScan"),
            "expected TronScan verification confirmation\nstderr: {stderr}"
        );
    }
);
