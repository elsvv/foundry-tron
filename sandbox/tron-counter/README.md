# tron-counter — Plan C E2E sandbox (`network = "tron"`)

Minimal Foundry project (no forge-std) that exercises the naive-stage Tron
network path in `forge`: `NetworkVariant::Tron` → `TronEvmNetwork` (vanilla
revm / Cancun, chain id 728126428).

## What is PROVEN working

1. **Native tron-solc 0.8.27, auto-resolved.** `foundry.toml` carries no `solc`
   key; on `network = "tron"` forge resolves the compiler through
   `foundry-tron-solc` into `~/.foundry-tron/solc/tron-solc-0.8.27`
   (github.com/tronprotocol/solidity, tag `tv_0.8.27`, universal macOS binary,
   sha256 `9e369b442b3a835320cf1450a21ce5e8acf0d319a50d10a10a2001aac7ce17aa`),
   downloading and pin-verifying it once if the cache is empty (skipped under
   `offline`). Runs **natively** on Apple Silicon (universal x86_64+arm64 binary
   — no Rosetta, no fallback). `--version` → `solc.tron ... 0.8.27+commit.19164bed`.
   An explicit `solc = "/abs/path"` still overrides the resolver for a locally
   built binary.

2. **`forge build` with real tron-solc succeeds.** Both contracts compile;
   trace confirms forge invokes the exact tron-solc binary via `--standard-json`
   with `"evmVersion":"cancun"` (exit 0). No svm/vanilla solc involved.

3. **Network plumbing is correct.** With the SAME sources compiled by **vanilla
   solc 0.8.27**, `forge test` on `network = "tron"` is **3/3 PASS**:
   - `testTronChainId`  → `block.chainid == 728126428` (chain id reaches EVM)
   - `testTransientStorageCancun` → TSTORE/TLOAD (Cancun spec active)
   - `testIncrement`    → deploy + call

## Blocker (RESOLVED in C2): tron-solc bytecode on the naive revm

> **Update (Plan C2):** the blocker below was closed by the mini `tron-revm`
> (`TronEvmFactory` with instruction stubs for `0xD0–0xD4`). `forge test` now
> runs the **real tron-solc bytecode** and passes **5/5**
> (`testIncrement`, `testTronChainId`, `testTransientStorageCancun`,
> `testNonPayableGuardWithTvmOpcodes`, `testCreate2TripleConsistency`). The
> original analysis is kept below for the record.

`forge test` with the **tron-solc-compiled** bytecode fails with
`EvmError: OpcodeNotFound`. Root cause: tron-solc injects TVM-only opcodes into
the non-payable entry guard of **every** contract, right after the standard
`CALLVALUE` check:

```
6080604052 34 80 15 <d> 57 5f 5f fd 5b 50   ; require(msg.value == 0)
           d3 80 15 <d> 57 5f 5f fd 5b 50   ; 0xD3 CALLTOKENID   -> require(tokenid == 0)
           d2 80 15 <d> 57 5f 5f fd 5b 50   ; 0xD2 CALLTOKENVALUE -> require(tokenvalue == 0)
```

`0xD2`/`0xD3` belong to the TVM range `0xD0–0xDF` (TRC-10 token opcodes).
Vanilla revm (Cancun) has no such opcodes, so execution halts on the first
`0xD3` encountered. There is **no tron-solc flag** to suppress this preamble
(`--help` has no token/tvm/payable toggle; `--evm-version` does not affect it).

This is exactly the Stage-1 limitation the design spec documents in §4.4:
*"опкоды 0xD0–0xDF не поддержаны"*. It directly collides with Plan C's Task 4
gate ("forge test 3/3 with tron-solc bytecode"): that gate is **not achievable**
at the naive stage. Executing tron-solc output requires Stage 2 (`tron-revm`,
custom instruction table for `0xD0–0xDF`), which is out of Plan C scope.

## Walkthrough

`FORGE` points at the forge binary you built from this repo (`cargo build -p
forge --bin forge` → `target/debug/forge`).

```bash
cd sandbox/tron-counter             # from the repo root
FORGE=../../target/debug/forge      # resolves to <repo>/target/debug/forge from here
$FORGE build                        # compiles with the real tron-solc (auto-resolved)
$FORGE test                         # 5/5 PASS on the tron-solc bytecode (mini tron-revm)
```

The committed `foundry.toml` has no `solc` key: on `network = "tron"` forge
auto-resolves the native tron-solc into `~/.foundry-tron/solc/tron-solc-0.8.27`
(downloading and sha256-pinning it once if the cache is empty; skipped under
`offline`). Compiling with the real tron-solc — TVM opcode preamble included —
is the whole point of the sandbox, and the mini tron-revm (`TronEvmFactory`)
executes that bytecode so `forge test` passes 5/5. To fall back to vanilla solc
for a plumbing-only check, add `solc = "0.8.27"` to `foundry.toml`.

See `docs/tron/USER_GUIDE.md` for the full Tron command reference (fork mode,
gas report, TronScan verification, config keys, address formats).

## Plan D — `cast` on Tron (offline utilities + Nile deploy/read)

With `network = "tron"` in `foundry.toml`, `cast send`/`call`/`balance` route
through the protobuf `/wallet/*` path instead of `eth_sendRawTransaction`.
`CAST=<repo>/target/debug/cast`.

Offline unit converters (no network):

```bash
$CAST to-sun 1.5                       # -> 1500000  (TRX -> SUN)
$CAST from-sun 1500000                 # -> 1.500000 (SUN -> TRX)
$CAST tron-address TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t   # base58 / 41-hex / 0x
```

Live on Nile (`set -a && source .env.tron-dev && set +a`, then `TRON_LIVE=1`):

```bash
cd sandbox/tron-counter

# Native TRX transfer (value is SUN): 1 TRX to another account. stdout = txID.
# The recipient MUST differ from the sender — Tron rejects a self-transfer at
# contract validation ("Cannot transfer TRX to yourself"). Confirmed on Nile:
# txID 9dc1b1d6af9071bd3b2b6b5ff24f5fb4226f6e8a3ab1206f4b4848c00093e8e0.
$CAST send TQuzjxWcqHSh1xDUw4wmMFmCcLjz4wSCBp --value 1000000 \
  --rpc-url nile --private-key $TRON_PRIVATE_KEY

# Balance (SUN, or TRX with --ether).
$CAST balance TX7izXWcmofRYonzdcThrS78jifMtVWCuf --rpc-url nile --ether

# Deploy the Counter creation bytecode; stdout = base58 contract address.
# NOTE: `--create` is a subcommand whose bytecode is a positional, so every flag
# (--tron.fee-limit, --rpc-url, --private-key) MUST come before `--create`;
# anything placed after it is rejected as an unexpected argument.
ADDR=$($CAST send --tron.fee-limit 400000000 --rpc-url nile --private-key $TRON_PRIVATE_KEY \
  --create $(cat ../../crates/evm/core/testdata/tron_counter_creation.hex))

# Contract call: setNumber(7) then read number() (constant call).
$CAST send $ADDR "setNumber(uint256)" 7 --rpc-url nile --private-key $TRON_PRIVATE_KEY
$CAST call $ADDR "number()(uint256)" --rpc-url nile          # -> 7
```

Addresses are accepted in `T…` (base58check), `41…`-hex and `0x…` forms on every
Tron path. `--tron.fee-limit` (SUN) and `--tron.expiration` (seconds) override the
`[tron]` config section.

## Plan D — `forge create` and `forge script --broadcast` (full Stage-1 cycle)

Both deploy through the protobuf `CreateSmartContract` / `TriggerSmartContract`
path (not `eth_sendTransaction`); the contract address is derived locally from
the txID via the java-tron `WalletUtil` formula and cross-checked against the
address the node reports once mined (mismatch = hard error).
`FORGE=<repo>/target/debug/forge`.

```bash
cd sandbox/tron-counter
set -a && source ../../.env.tron-dev && set +a   # loads TRON_PRIVATE_KEY

# forge build (real tron-solc) and forge test (5/5 on tron-solc bytecode).
$FORGE build
$FORGE test

# forge create: compiles src/Counter.sol, appends ABI-encoded ctor args (none
# here), deploys. stdout carries deployer/address/txID; --broadcast is required
# to touch the network (omit it for a dry run that prints the creation bytecode).
$FORGE create src/Counter.sol:Counter --rpc-url nile \
  --private-key "$TRON_PRIVATE_KEY" --tron.fee-limit 400000000 --broadcast

# forge script: runs Deploy.s.sol (new Counter() + counter.setNumber(42) under a
# broadcast) and replays the collected CreateSmartContract + TriggerSmartContract
# to Nile. The on-chain (fork) simulation phase is skipped — forking a Tron node
# needs eth_ JSON-RPC (Stage 2). Artifacts land under
# broadcast/Deploy.s.sol/3448148188/ (Nile chain id).
$FORGE script script/Deploy.s.sol --rpc-url nile --broadcast \
  --private-key "$TRON_PRIVATE_KEY" --tron.fee-limit 400000000
```

The broadcast artifact `broadcast/Deploy.s.sol/3448148188/run-latest.json` keeps
the standard Foundry shape; `hash` carries the Tron txID, and each transaction
gains an optional `tron` block:

```json
{ "txid": "0x2507a88a…", "ownerBase58": "TX7izXWcmof…",
  "contractAddressBase58": "THhVv6vwHsy…",
  "feeLimit": 400000000, "energyUsed": 101188, "feeSun": 10962800 }
```

`--verify`, `--unlocked` and `--browser` on `forge create` are rejected up front
("not supported on tron yet"); `--fork-url` on `forge script` is a Stage-2 error.

### Live-run evidence (Nile, chain id 3448148188, 2026-07-12)

| Step | txID | Result |
|---|---|---|
| `forge create Counter` | `6a1b82bcb399afafb5084cec1cae2f244b6f2baa33a878649837589126b51fbb` | → `TEsgXDHsYDMvdpoAswPuPDeghuUiAucivJ` (`4135cd1b0867d8a1d16be5a93718b63ae0392ef14d`), 10.96 TRX, 101188 energy |
| `cast send setNumber(7)` | `d73d8c9315557d24954157858052ec7f627966148b42c72934fd8f39f3491fb6` | block 69109676; `number()` → 7 |
| `cast send` transfer 1 TRX | `059bedb74d019ae58a1de528f53938e1faf6a88a2af59ab565cda7b2b4870d26` | block 69109682, 0.274 TRX |
| `forge script` CREATE | `2507a88a8120adeb43f1eea47e53452656dbe57149e1a0e9c8e8c449753292b1` | → `THhVv6vwHsyaKm42hPUHVc8szHy4xNPagt` (`4154c8786eb31e9d5d76aafc1d46742acdda711084`), 10.96 TRX |
| `forge script` CALL setNumber(42) | `cdc904f3cc87f622fb5ec67a3606b5a52d288e59a276462dfda589e25ad0d12f` | `number()` → 42 |

The public Nile endpoint (`nile.trongrid.io`, no API key) WAF-throttles a burst
of `/wallet/*` POSTs with HTTP 405; set `TRON_PRO_API_KEY` to avoid it.

## Plan G — read-only fork over `/jsonrpc`

`forge test --fork-url <host>/jsonrpc` with `network = "tron"` forks real Tron
state into tron-revm (Plan E energy model + precompiles). It is **read-only** —
no transaction is broadcast, no TRX is spent — and **mainnet-only**: the live
`/jsonrpc` servlet is mounted on `api.trongrid.io` but **not** on `api.nileex.io`
(nginx 404), and it is inherently **tip-only** (java-tron serves account/storage
state only at the `latest` tag; a pinned block number returns `-32602`).

Two shims on the foundry side make it work (java-tron's `/jsonrpc` intentionally
answers `eth_getTransactionCount` with a permanent `-32601`, and rejects
block-number state queries): a nonce shim (`eth_getTransactionCount → 0x0`) and a
tip-only unpin of the fork state block. Both `--fork-url` (global fork) and
`vm.createSelectFork` route through them.

```bash
cd sandbox/tron-counter
FORGE=<repo>/target/debug/forge

# Read the real mainnet USDT contract on a fork (drop this test into test/):
cat > test/UsdtFork.t.sol <<'SOL'
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;
interface Vm { function createSelectFork(string calldata) external returns (uint256); }
interface IERC20 { function name() external view returns (string memory);
                   function decimals() external view returns (uint8);
                   function totalSupply() external view returns (uint256); }
contract UsdtForkTest {
    Vm constant vm = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);
    IERC20 constant USDT = IERC20(0xa614f803B6FD780986A42c78Ec9c7f77e6DeD13C);
    function test_fork_usdt() public {
        vm.createSelectFork("https://api.trongrid.io/jsonrpc");
        require(keccak256(bytes(USDT.name())) == keccak256("Tether USD"), "name");
        require(USDT.decimals() == 6, "decimals");
        require(USDT.totalSupply() > 0, "supply");
    }
}
SOL
TRON_LIVE=1 $FORGE test --mt test_fork_usdt -vvv

# Or a global fork from the command line:
TRON_LIVE=1 $FORGE test --fork-url https://api.trongrid.io/jsonrpc -vvv
```

Canonical, gated coverage lives in `crates/forge/tests/cli/tron.rs`
(`tron_mainnet_fork_reads_usdt`, `TRON_LIVE=1`): it also cross-checks a raw
balances storage slot (`keccak256(abi.encode(holder, 0))`) against `balanceOf`
and confirms `extcodesize > 0`. Forking a Tron node under `forge script`
(broadcast) is still rejected with an actionable error.
