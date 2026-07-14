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
