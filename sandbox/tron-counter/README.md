# tron-counter — Plan C E2E sandbox (`network = "tron"`)

Minimal Foundry project (no forge-std) that exercises the naive-stage Tron
network path in `forge`: `NetworkVariant::Tron` → `TronEvmNetwork` (vanilla
revm / Cancun, chain id 728126428).

## What is PROVEN working

1. **Native tron-solc 0.8.27** downloaded to
   `~/.foundry-tron/solc/tron-solc-0.8.27`
   (github.com/tronprotocol/solidity, tag `tv_0.8.27`, universal macOS binary,
   sha256 `9e369b442b3a835320cf1450a21ce5e8acf0d319a50d10a10a2001aac7ce17aa`).
   Runs **natively** on Apple Silicon (universal x86_64+arm64 binary — no
   Rosetta, no fallback). `--version` → `solc.tron ... 0.8.27+commit.19164bed`.

2. **`forge build` with real tron-solc succeeds.** Both contracts compile;
   trace confirms forge invokes the exact tron-solc binary via `--standard-json`
   with `"evmVersion":"cancun"` (exit 0). No svm/vanilla solc involved.

3. **Network plumbing is correct.** With the SAME sources compiled by **vanilla
   solc 0.8.27**, `forge test` on `network = "tron"` is **3/3 PASS**:
   - `testTronChainId`  → `block.chainid == 728126428` (chain id reaches EVM)
   - `testTransientStorageCancun` → TSTORE/TLOAD (Cancun spec active)
   - `testIncrement`    → deploy + call

## Blocker: tron-solc bytecode does NOT run on the naive revm

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

## Reproduce

```bash
FORGE=/Users/vaceslaveliseev/@dev/foundry-tron/foundry/target/debug/forge
cd /Users/vaceslaveliseev/@dev/foundry-tron/sandbox/tron-counter
$FORGE build              # OK: compiles with tron-solc
$FORGE test  -vv          # FAIL: EvmError: OpcodeNotFound (0xD3 CALLTOKENID)

# Plumbing proof (vanilla solc): edit foundry.toml -> solc = "0.8.27", then:
$FORGE test  -vv          # 3/3 PASS
```

The committed `foundry.toml` keeps `solc = <tron-solc>` because compiling with
real tron-solc is the point of the sandbox; swap to `solc = "0.8.27"` to see the
3/3 plumbing pass.

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
