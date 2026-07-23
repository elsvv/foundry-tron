# Foundry for Tron — User Guide

This is a fork of [Foundry](https://github.com/foundry-rs/foundry) that adds
first-class support for the [Tron](https://tron.network) network (TVM). It keeps
the entire Ethereum toolchain intact and adds a Tron execution path that is
selected explicitly with a single config key: `network = "tron"`.

Everything here is a fork addition. When `network = "tron"` is **not** set,
`forge`, `cast`, `anvil` and `chisel` behave exactly like upstream Foundry.

- **Tron mainnet** chain id: `728126428` (`0x2b6653dc`)
- **Tron Nile testnet** chain id: `3448148188` (`0xcd8690dc`)

> This build identifies itself in `forge --version` / `cast --version` with a
> `tron` marker carrying the Tron toolchain version (e.g.
> `forge 1.7.2-dev (tron 0.2.0; <sha> <ts>)`), so you can tell a Tron build apart
> from an upstream one and see which fidelity stage it carries.

---

## Contents

1. [Quickstart](#quickstart)
2. [Command coverage matrix](#command-coverage-matrix)
3. [`[tron]` configuration reference](#tron-configuration-reference)
4. [Address formats](#address-formats)
5. [The tron-solc compiler](#the-tron-solc-compiler)
6. [Gas report: energy and bandwidth](#gas-report-energy-and-bandwidth)
7. [Dynamic energy (TIP-491)](#dynamic-energy-tip-491)
8. [Estimating cost (`cast estimate`)](#estimating-cost-cast-estimate)
9. [Fork mode (read-only, tip-only)](#fork-mode-read-only-tip-only)
10. [Contract verification (TronScan)](#contract-verification-tronscan)
11. [VM and energy-model differences](#vm-and-energy-model-differences)
12. [Live-test environment gates](#live-test-environment-gates)
13. [Installing from the fork](#installing-from-the-fork)

---

## Quickstart

A Tron project is an ordinary Foundry project with `network = "tron"` in
`foundry.toml`. That one key routes builds, tests, deploys and verification
through the Tron path.

```toml
# foundry.toml
[profile.default]
src = "src"
out = "out"
test = "test"
network = "tron"          # <- selects the Tron execution + broadcast path
chain_id = 728126428      # 728126428 = mainnet, 3448148188 = Nile testnet
evm_version = "cancun"    # TVM (java-tron 4.8.x) is Cancun-equivalent
# No `solc` key: forge auto-resolves the native tron-solc compiler (see below).

[rpc_endpoints]
mainnet = "https://api.trongrid.io"     # /wallet writes; /jsonrpc reads (fork)
nile    = "https://api.nileex.io"        # Nile testnet, /wallet only (no /jsonrpc)
```

Then:

```sh
forge build         # compiles with the native tron-solc, auto-resolved
forge test          # runs against tron-revm (energy model + TVM precompiles)
```

A ready-made example project lives in [`sandbox/tron-counter`](../../sandbox/tron-counter)
(no `forge-std`; it asserts `block.chainid == 728126428` and exercises Cancun
transient storage). See its `README.md` for a full build/test/deploy
walkthrough.

Deploying to Nile (the write path goes over the protobuf `/wallet/*` API, not
`eth_sendRawTransaction`):

```sh
# Load a funded Nile key (base58 T-address holds test TRX from the faucet).
export TRON_PRIVATE_KEY=<hex-private-key>

forge create src/Counter.sol:Counter \
  --rpc-url nile --private-key "$TRON_PRIVATE_KEY" \
  --tron.fee-limit 400000000 --broadcast
```

`cast` works the same way once `network = "tron"` is set:

```sh
cast to-sun 1.5                          # 1500000   (TRX -> SUN)
cast from-sun 1500000                    # 1.500000  (SUN -> TRX)
cast tron-address TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t  # convert address forms
cast balance TX7izXWcmof... --rpc-url nile --ether    # balance in TRX
cast call <addr> "number()(uint256)" --rpc-url nile   # constant call
```

---

## Command coverage matrix

Derived from the project status log and the code. "Works" means it is exercised
by tests and/or confirmed live on Nile/mainnet.

### Works on Tron

| Command | Notes |
|---|---|
| `forge build` | Auto-resolves and compiles with the native tron-solc. |
| `forge test` | Runs on `tron-revm`: faithful energy model, java-tron precompiles, Tron CREATE2, Cancun feature set. |
| `forge test --gas-report` | Report is relabeled to **energy** and gains **bandwidth** columns (see [Gas report](#gas-report-energy-and-bandwidth)). |
| `forge test --fork-url <host>/jsonrpc` | Read-only, tip-only, **mainnet only** (see [Fork mode](#fork-mode-read-only-tip-only)). |
| `forge snapshot`, `forge coverage` | Share the `forge test` execution path; same Tron behavior. |
| `forge create` | Deploys via `CreateSmartContract`; the contract address is derived locally and cross-checked against the node. |
| `forge create --verify` | Deploy **and** verify on TronScan in one command (a pre-broadcast preflight validates compiler pin / host / flatten before spending TRX). |
| `forge script --broadcast` | Replays collected `CreateSmartContract` / `TriggerSmartContract` transactions; artifact keeps the standard Foundry shape plus an optional `tron { ... }` block. |
| `forge verify-contract` | Standalone TronScan verification (see [Verification](#contract-verification-tronscan)). |
| `cast to-sun` / `from-sun` / `tron-address` | Offline unit and address converters. |
| `cast call` / `send` / `balance` | Route through the protobuf `/wallet/*` path; `send --create <bytecode>` deploys; `balance --ether` prints TRX. |

### Explicit error (guarded, not silent)

| Command | Error |
|---|---|
| `forge create --unlocked` | `--unlocked is not supported on tron yet` |
| `forge create --browser` | `--browser is not supported on tron yet` |
| `forge script --verify` | Not supported on Tron yet (script verification does not map cleanly onto the TronScan provider; use `forge verify-contract` or `forge create --verify` instead). |
| `forge script --fork-url ...` | Forking a Tron node is rejected under `forge script` (broadcast context); use `forge test --fork-url .../jsonrpc` for a read-only fork. |
| `forge test/snapshot/coverage --fork-url ... --fork-block-number <N>` | `Tron forks are tip-only: state is served only at the chain tip (/jsonrpc serves state only at TAG latest). Drop --fork-block-number for a tip fork.` |
| `vm.createFork(url, block)` / `vm.createSelectFork(url, block)` | Same tip-only explanation, as a cheatcode revert. The block-less variants are unaffected. |

### Out of scope

- `anvil` — no local Tron node (Tron is protobuf, not RLP; there is no
  `eth_sendRawTransaction`).
- `chisel` — the Solidity REPL is Ethereum-only.
- The TUI debugger.

---

## `[tron]` configuration reference

The `[tron]` section of `foundry.toml` carries the Tron-specific transaction
parameters. Values are raw protobuf fields; the defaults below come straight
from `crates/config/src/tron.rs`.

| Key | Default | Units | Maps to |
|---|---|---|---|
| `fee_limit` | `1_000_000_000` (1000 TRX) | SUN | `Transaction.raw_data.fee_limit` — the maximum TRX (in SUN) that may be burned for one transaction. |
| `origin_energy_limit` | `10_000_000` | energy | `SmartContract` tag 8 — energy the contract owner contributes on deployment. |
| `user_fee_percentage` | `100` | percent (0–100) | `SmartContract.consume_user_resource_percent` (tag 6) — share of energy paid by the caller. |
| `expiration` | `60` | seconds | Added to the current time when building a transaction (converted to ms by the sender). |
| `dynamic_energy` | `true` | bool | Whether `forge test --gas-report` models the TIP-491 dynamic-energy penalty on a Tron fork (see [Dynamic energy](#dynamic-energy-tip-491)). No effect off Tron or on a non-fork run. |

`1 TRX = 1_000_000 SUN`.

Example:

```toml
[tron]
fee_limit = 1000000000
origin_energy_limit = 10000000
user_fee_percentage = 100
expiration = 60
dynamic_energy = true
```

### Per-command overrides

Two of these can be overridden per command without editing `foundry.toml`:

- `--tron.fee-limit <SUN>`
- `--tron.expiration <SECONDS>`

```sh
forge create src/Counter.sol:Counter --rpc-url nile \
  --private-key "$TRON_PRIVATE_KEY" --tron.fee-limit 400000000 --broadcast
```

---

## Address formats

Every Tron path accepts an address in three interchangeable forms:

- **base58check** — `T...` (the canonical Tron display form)
- **41-hex** — `41` followed by the 20-byte hex address (Tron's on-chain 21-byte form)
- **0x-hex** — the plain 20-byte EVM form, `0x...`

When `network = "tron"`, addresses are **printed** in base58 (`T...`). Converters
and address-taking arguments (including `forge verify-contract <address>` and
`cast tron-address`) accept all three forms.

### base58 in traces

On a Tron run, `forge test -vvvvv`, `forge script`, `cast run`/`cast call --trace`
and the chisel REPL render addresses in base58:

- An **unlabeled contract** is identified in the trace tree by its base58 form, so
  a node reads `TEsg…9Yb::setNumber(...)` instead of `0x…::setNumber`. An explicit
  `vm.label` and known project contracts keep their name (they win over the base58
  fallback).
- Every **address value in decoded call arguments, returns and event logs** —
  including addresses nested inside arrays, tuples and structs — prints base58.

`console.log` output is formatted by the Solidity library, not the decoder, so it
stays hex; use `cast tron-address <0x… | 41… | T…>` to convert any form to base58.
Off Tron, traces are byte-for-byte unchanged.

---

## The tron-solc compiler

Tron's solc emits TVM-only opcodes (the `0xD0–0xDF` range, e.g. `CALLTOKENID` /
`CALLTOKENVALUE`) into the non-payable guard of **every** contract. That
bytecode does not run on a vanilla EVM, and the standard `svm` compiler manager
cannot fetch or checksum Tron binaries. This fork solves both:

- On `network = "tron"` with no `solc` key, forge auto-resolves the native
  tron-solc from the `tronprotocol/solidity` GitHub releases into
  `~/.foundry-tron/solc/tron-solc-<version>`, verifying it against a pinned
  sha256 the first time (skipped when `offline`). The default version is
  **0.8.28**; pinned versions are **0.8.23 – 0.8.28** for linux-amd64 /
  macos (universal) / windows-amd64.
- **Choosing a version:** set `solc = "X.Y.Z"` in `foundry.toml` to pin a
  specific tron-solc. A pinned version resolves offline against its compiled-in
  sha256. A version **newer than this toolchain release** (not in the pin table)
  is resolved online from the official `tronprotocol.github.io/solc-bin` list and
  verified against the `sha256` that list publishes — so you can move to a
  freshly released tron-solc without waiting for a toolchain update. With
  `offline = true` an unpinned version fails deterministically instead of
  reaching the network.
- On Apple Silicon the macOS binary is a universal build — no Rosetta.
- An explicit `solc = "/abs/path"` still overrides the resolver entirely so you
  can point at a locally built compiler.
- `tron-revm` then executes the resulting TVM bytecode, so `forge test` runs the
  real tron-solc output rather than a vanilla-solc stand-in.

There is no native `linux-arm64` tron-solc release; on Linux ARM (and any other
non-macOS/Windows/linux-x86_64 target) the resolver returns a clear
`UnsupportedPlatform` error (build from source or supply an explicit `solc`).
Apple Silicon is unaffected — macOS aarch64 is served by the universal
`solc-macos` binary above.

---

## Gas report: energy and bandwidth

Tron meters execution in **energy** (its analogue of EVM gas) and separately
charges **bandwidth** in bytes for the serialized transaction. On the Tron path,
`forge test --gas-report` reflects both:

- The `gas` figures are **energy** (`trace.gas_used` is TVM energy under the
  faithful energy model; the intrinsic cost is bandwidth, not energy, and is
  zeroed). The deployment row is relabeled **Deployment Energy**, and per-call
  columns are energy.
- Each contract gains a **Deployment Bandwidth** cell and each function gains a
  **Bandwidth Min / Avg / Median / Max** block (bytes).

Bandwidth is an estimate for the transaction this toolchain broadcasts:

```
bandwidth (bytes) = serialized_size(signed protobuf Transaction, ret cleared) + 64
```

The `+ 64` is java-tron's `MAX_RESULT_SIZE_IN_TX`; the signature is a fixed 65
bytes. This has been validated live to the byte (a real mainnet
`transfer(address,uint256)` → 345 bytes, matching the node's `net_fee` of
345000 SUN at 1000 SUN/byte). The estimate is exact for transactions built by
this fork's broadcast path; wallets that populate extra protobuf fields (e.g.
`ref_block_num`) may differ by a few bytes.

The `--json` output adds these fields only on the Tron path, guarded so that
EVM `--gas-report --json` output is byte-for-byte unchanged.

`forge snapshot` is unaffected — it parses the `(gas: N)` value from result
text, which on Tron already carries energy.

---

## Dynamic energy (TIP-491)

Tron charges "hot" contracts extra energy. Under TIP-491 (Proposal #83, live since
2023) a contract that keeps executing more than `getDynamicEnergyThreshold`
(5,000,000,000) energy per maintenance cycle accrues a per-contract **energy
factor**; each maintenance cycle (6h) moves the factor up or down, capped at
`getDynamicEnergyMaxFactor` (34000, precision 10000 → 3.4x). The charged energy is:

```
charged = base × (1 + factor / 10000)
```

applied to the energy a contract executes **in its own context** (its own
instructions, not its sub-calls). USDT (`TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t`) sits
at the maximum, so a `transfer()` costs up to 4.4x its base energy.

**Where this shows up in the toolchain:**

- **`forge test --gas-report` on a Tron fork** models the penalty. For every
  contract in the run it fetches the live energy factor from the fork node
  (`/wallet/getcontractinfo`) and, when a factor is present, adds a **Penalty Avg**
  column to the function table and a **Deployment Penalty** cell (JSON:
  `energy_penalty`). The base **Energy** columns stay base energy; the penalty is
  reported alongside. Toggle with `[tron] dynamic_energy` (default `true`); a
  non-fork run (fresh, factor-less contracts) leaves the report base-energy only.
- **`cast estimate`** reports the penalty portion of a call's energy (see below).

**Limitation:** the penalty is computed after the fact from the trace, not inside
the interpreter, so a contract's own `gasleft()` inside a hot frame reflects base
energy, not the charged total. Estimates and the gas-report penalty column are
correct; only in-contract `gasleft()` introspection is base-only.

---

## Estimating cost (`cast estimate`)

`cast estimate <contract> "<sig>" [args…] --from <addr>` estimates a call's Tron
cost. Energy comes from the node's `/wallet/estimateenergy` when it is enabled,
otherwise from `/wallet/triggerconstantcontract` (whose `energy_used` already
includes the TIP-491 penalty); bandwidth is estimated from the call's calldata,
and the live economics come from `/wallet/getchainparameters`.

Text output (stdout is the result; `--json` emits the same fields):

```
energy used:         64285
energy penalty:      49635
bandwidth (bytes):   345
suggested fee_limit: 7714200 SUN
est. cost:           7.71 TRX
```

The suggested `fee_limit` applies a safety buffer and is clamped to the node's
ceiling (`getMaxFeeLimit`, 15,000 TRX):

```
suggested_fee_limit = min(energy × energy_price × (100 + buffer) / 100,
                          getMaxFeeLimit)
```

with a 20% buffer by default and `energy_price = getEnergyFee` (100 SUN). The
broadcast paths (`cast send`, `forge create`, `forge script`) reject a `fee_limit`
above `getMaxFeeLimit` before sending, with a hint to lower `--tron.fee-limit`.
`base energy = energy_used − energy_penalty`.

---

## Fork mode (read-only, tip-only)

`forge test --fork-url <host>/jsonrpc` with `network = "tron"` forks real Tron
state into `tron-revm` (the same energy model and precompiles used for local
tests). Three properties follow from java-tron's `/jsonrpc` servlet:

- **Read-only.** No transaction is broadcast; no TRX is spent.
- **Mainnet only.** `/jsonrpc` is mounted on `https://api.trongrid.io` but not on
  `https://api.nileex.io` (Nile serves only `/wallet/*`).
- **Tip-only.** java-tron serves account/storage/code **only** at the `latest`
  tag; a pinned block number is rejected. State therefore always reflects the
  current chain tip.

Because of tip-only state, passing an explicit historical `--fork-block-number`
would produce a silent mix of pinned block env and tip state. That case is now a
**hard error**:

```
Tron forks are tip-only: state is served only at the chain tip
(/jsonrpc serves state only at TAG latest). Drop --fork-block-number for a tip fork.
```

The same guard applies to `forge snapshot` / `forge coverage` and to the
cheatcodes `vm.createFork(url, block)` / `vm.createSelectFork(url, block)`. The
block-less fork forms (a tip fork) stay green.

Example (read the live mainnet USDT contract on a fork):

```sh
TRON_LIVE=1 forge test --fork-url https://api.trongrid.io/jsonrpc -vvv
```

---

## Contract verification (TronScan)

Source verification goes to **TronScan** (Etherscan is Ethereum-only). The
TronScan verify endpoint is **keyless** and **synchronous** (no API key, no GUID
polling), and accepts a single **flattened** Solidity file.

Standalone verification:

```sh
forge verify-contract <address> src/Counter.sol:Counter \
  --license-type MIT \
  --watch
```

- `<address>` accepts `T...`, `41...` or `0x...`.
- The Tron path is selected by `network = "tron"`; the host is chosen from the
  chain id (mainnet or Nile), or overridden with `--verifier-url <https://host>`.
- `--verifier tronscan` selects the provider explicitly if needed.
- `--watch` polls TronScan's `/info` endpoint until the contract reports
  `status == 2` (verified).

One-shot deploy + verify:

```sh
forge create src/Counter.sol:Counter --rpc-url nile \
  --private-key "$TRON_PRIVATE_KEY" --broadcast \
  --verify --license-type MIT
```

### Hosts

| Network | Chain id | API host | Contract page |
|---|---|---|---|
| Mainnet | `728126428` | `https://apilist.tronscanapi.com` | `https://tronscan.org/#/contract/<addr>` |
| Nile | `3448148188` | `https://nileapi.tronscan.org` | `https://nile.tronscan.org/#/contract/<addr>` |

If the chain id cannot be determined, verification asks you to pass
`--verifier-url` explicitly.

### License codes

`--license-type` accepts either an SPDX name (e.g. `MIT`) or the numeric
Etherscan license code that TronScan expects. Common codes: **3 = MIT**,
**12 = Apache-2.0**, **14 = BUSL-1.1**. When omitted, the submission defaults to
`1` (No License).

### Compiler string

TronScan stores the compiler as `tron_v<solc long version>` (e.g.
`tron_v0.8.27+commit.19164bed`). The fork resolves the correct long-version
commit from a pinned table (source: `tronprotocol.github.io/solc-bin`
`list.json`) because the tron-solc resolver does not exec `--version`.

---

## VM and energy-model differences

`tron-revm` reproduces java-tron 4.8.x semantics on top of a Cancun base. Notable
differences from a stock EVM, worth knowing when a local result and an on-chain
result could diverge:

- **Energy model.** Tron uses pre-EIP-150 (FRONTIER) gas as a base plus TVM
  deltas, applied as a data override on Cancun execution. Refunds are disabled
  (Tron has none), EIP-150's 63/64 call-gas rule is off, and EIP-3860 per-word
  init-code metering and the EIP-170 code-size cap are removed. Golden energy
  parity is confirmed exact against Nile (read `number()` = 414, write
  `setNumber(7)` = 20438 energy).
- **Chain id.** A local (non-fork) `network = "tron"` run defaults `block.chainid`
  to Tron mainnet (`728126428`), so EIP-712 / permit domains resolve without
  pinning it. Set `chain_id = 3448148188` in `foundry.toml` to target Nile; a fork
  takes the node's id.
- **BASEFEE / `vm.fee`.** `block.basefee` returns `getEnergyFee()` (100 sun) by
  default — Tron's BASEFEE opcode is the energy price, not the Ethereum base fee.
  `vm.fee(x)` overrides it like on any other network. On a fork the local energy
  price (100 sun) is used, not the `/jsonrpc` block's Ethereum `baseFee`.
- **COINBASE.** Locally `block.coinbase` defaults to `0x0` and is settable with
  `vm.coinbase` (env-driven, works). On live Tron it is the block's SR address in
  `0x41`-prefixed form; foundry does not override it.
- **No-op cheatcodes.** `vm.prevrandao` and `vm.txGasPrice` have no observable
  effect on Tron because the TVM hardwires the PREVRANDAO/DIFFICULTY and GASPRICE
  opcodes to `0`. Calling them is not an error; foundry prints a one-time stderr
  warning per run so the no-op is visible rather than silent. `vm.difficulty` is
  not in this group: as on any post-Merge chain it hard-errors (`use 'prevrandao'
  instead`), because Tron runs a Cancun-based spec — so it never reaches, and
  never prints, the no-op warning.
- **CREATE2** uses the Tron formula:
  `address = keccak256(0x41 ‖ sender20 ‖ salt ‖ keccak256(initcode))[12..]`, and
  the `computeCreate2Address*` cheatcodes are specialized accordingly.
- **CREATE (internal)** still uses the EVM address scheme, not Tron's
  `keccak(rootTxId ‖ nonce)` — the root tx id is not reproducible locally. This
  is a documented local delta.
- **Call depth** is EVM's 1024 locally versus Tron's 64 on-chain; a contract that
  recurses deeper than 64 passes locally but reverts on-chain.
- **Precompiles** follow java-tron's table: `0x03` is a double-sha256 (not
  ripemd160), `0x09` / `0x0a` are TIP-43 / TIP-60 signature validators (replacing
  blake2f / KZG), real ripemd160 and blake2f move to `0x020003` / `0x020009`.
- **ISCONTRACT (`0xD4`)** is a stub that checks for a non-empty code account;
  it can differ from java-tron's contract-account check in edge cases (e.g. a
  self-check inside a constructor).
- Trace mnemonics for `0xD0–0xD4` still render as unknown opcodes (cosmetic).

Contract addresses for real deploys are computed as
`keccak256(txID ‖ owner21)[12..]` (java-tron `WalletUtil.generateContractAddress`,
where `txID = sha256(raw_data)` and `owner21 = 0x41 ‖ owner20`) and hard-checked
against the address the node reports once mined.

---

## Live-test environment gates

Tests that touch the network are gated behind environment variables and skip
cleanly (with an explicit message) when unset, so a normal `cargo test` /
`forge test` run spends no TRX and needs no network:

| Variable | Enables |
|---|---|
| `TRON_LIVE=1` | Read-only live tests (mainnet `/jsonrpc` fork, Nile read probes). |
| `TRON_PRIVATE_KEY` | Write-path live tests (deploy / send). Must be a funded Nile key. |
| `TRON_PRO_API_KEY` | A TronGrid API key to avoid WAF throttling on `/wallet/*` bursts. |
| `TRON_VERIFY_E2E=1` | The deploy-and-verify acceptance test (spends TRX; also needs `TRON_LIVE=1` + `TRON_PRIVATE_KEY`). |
| `TRON_SOLC_DOWNLOAD=1` | The test that actually downloads a tron-solc binary and checks its pin. |

Nile faucet: <https://nileex.io/join/getJoinPage>.

---

## Installing from the fork

### Prebuilt binaries (one-line installer)

The fastest path. It fetches the four Tron CLIs (`forge-tron`, `cast-tron`,
`anvil-tron`, `chisel-tron`) for your OS/arch from the rolling
`foundry-tron-latest` release, verifies the SHA-256 checksum, and unpacks them
into `~/.foundry-tron/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/elsvv/foundry-tron/master/install-foundry-tron.sh | bash
```

On networks where `raw.githubusercontent.com` is blocked (some corporate or
regional networks), pull the script from the jsDelivr mirror instead:

```sh
curl -fsSL https://cdn.jsdelivr.net/gh/elsvv/foundry-tron@master/install-foundry-tron.sh | bash
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/elsvv/foundry-tron/master/install-foundry-tron.ps1 | iex
```

The installer downloads with plain `curl` and transparently falls back to
`gh release download` when the release asset host itself is blocked (the CDN
above only serves the script). Flags:

- `--modify-path` (`-ModifyPath` on Windows): append the bin directory to your
  shell profile. By default the installer leaves `PATH` alone and just prints the
  line to add.
- `--dir <path>`: install somewhere other than `~/.foundry-tron`.
- `--require-checksum`: fail instead of warning when a release has no checksum.

Re-runs are idempotent, so the same command upgrades an existing install. The
binaries carry the `-tron` suffix to coexist with a stock Foundry install; add
`~/.foundry-tron/bin` to `PATH` and call `forge-tron`, `cast-tron`, etc.

### With foundryup

`foundryup` is parameterized by `FOUNDRYUP_REPO`, so the standard installer can
target this fork (this builds the stock `forge`/`cast` names, not the `-tron`
suffixed ones):

```sh
FOUNDRYUP_REPO=elsvv/foundry-tron foundryup
```

Or build from source:

```sh
git clone https://github.com/elsvv/foundry-tron
cd foundry-tron
cargo build --release --bins        # forge, cast, anvil, chisel
```

A Tron build reports a `tron` marker in its version string
(`forge --version` / `cast --version`), so you can confirm you are running the
fork.
