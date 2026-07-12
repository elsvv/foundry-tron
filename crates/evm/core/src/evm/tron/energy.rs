//! Faithful TVM energy model for tron-revm.
//!
//! Tron's energy metering is pre-EIP-150 (FRONTIER) Ethereum gas plus a small
//! set of TVM deltas. revm 41 factored gas out of opcode logic into three
//! overridable data surfaces -- the static `GasTable`, the dynamic `GasParams`,
//! and per-op instruction fns -- so a faithful energy model is a *data*
//! substitution, not a logic rewrite. We keep execution on CANCUN (all opcodes
//! and journaling stay live) and only swap the gas data:
//!
//! 1. replace the static gas table wholesale with the FRONTIER baseline (`gas_table()`), which
//!    already matches java-tron `EnergyCost.java` for SLOAD=50, BALANCE=20, EXTCODEHASH=400,
//!    CALL=40, TLOAD/TSTORE=100, LOG=375, KECCAK256=30, EXP base=10, and the memory formula `3w +
//!    w^2/512`;
//! 2. set the dynamic `GasParams` to the FRONTIER spec, then override the few Tron-specific deltas
//!    (see [`apply_tron_energy`]).
//!
//! Sources (java-tron @develop, fetched via jsdelivr on 2026-07-12):
//! `actuator/src/main/java/org/tron/core/vm/EnergyCost.java`,
//! `.../program/Program.java`,
//! `chainbase/src/main/java/org/tron/common/runtime/ProgramResult.java`.

use alloy_evm::{Database, eth::EthEvmContext, precompiles::PrecompilesMap};
use revm::{
    bytecode::opcode,
    context::Evm as RevmEvm,
    context_interface::cfg::{GasId, GasParams},
    handler::{EthFrame, instructions::EthInstructions},
    interpreter::{gas_table, interpreter::EthInterpreter},
    primitives::hardfork::SpecId,
};

/// Tron `getEnergyFee()` in SUN per energy unit. Probed live on both mainnet
/// (`api.trongrid.io`) and Nile (`api.nileex.io`) on 2026-07-12; identical on
/// both. This is what BASEFEE (0x48) returns on Tron, and the price used to
/// convert energy to burned TRX. (The design doc's 420 was incorrect.)
pub const TRON_ENERGY_FEE_SUN: u64 = 100;

/// Tron SUICIDE_V2 static energy (`EnergyCost.java` `SUICIDE_V2`), active under
/// `allowTvmSelfdestructRestriction` (on since mainnet 4.8). FRONTIER revm
/// charges 0 statically, so we bump it.
const TRON_SELFDESTRUCT_ENERGY: u16 = 5000;

/// Tron NEW_ACCT_CALL energy charged by SUICIDE_V2 when the inheritor is a dead
/// account (`EnergyCost.java` `getSuicideCost*`), matching `gas::NEWACCOUNT`.
const TRON_SELFDESTRUCT_NEW_ACCOUNT_ENERGY: u64 = 25_000;

/// Tron TLOAD/TSTORE energy (`EnergyCost.java` `TLOAD`/`TSTORE`). Already 100 in
/// the FRONTIER base table, re-asserted here for clarity/robustness.
const TRON_TRANSIENT_ENERGY: u16 = 100;

/// Applies the faithful TVM energy model to a freshly built revm EVM: swaps the
/// static gas table to the FRONTIER baseline, re-asserts the TVM static deltas,
/// installs FRONTIER `GasParams`, and overrides the Tron-specific dynamic
/// deltas. Also lifts the Ethereum contract-size limits Tron does not have.
///
/// This must run before any `insert_instruction` for the TVM/block opcodes,
/// because the wholesale `gas_table` replacement would otherwise clobber their
/// static gas entries.
///
/// # Refund semantics (verified 2026-07-12)
///
/// Tron has **no** EVM gas-refund counter. In `ProgramResult.java` the
/// `futureRefund` field (`:25`) and its accessors `addFutureRefund` /
/// `getFutureRefund` / `resetFutureRefund` (`:221-230`) are all commented out,
/// as are `Program.futureRefundEnergy` / `resetFutureRefund`
/// (`Program.java:1272-1278`). `ProgramResult.refundEnergy(long)` (`:82`) only
/// subtracts *unused* energy left over from message calls -- it is not the
/// SSTORE-clear / SELFDESTRUCT gas-refund mechanism -- and `EnergyCost.java`
/// records no refund at all. So clearing a storage slot (x->0), restoring one
/// in-tx (0->x->0), and SELFDESTRUCT all accrue no refund on Tron. revm still
/// computes refunds from `GasParams`, so every refund-only GasId is zeroed
/// below: `sstore_clearing_slot_refund`, `sstore_set_refund`,
/// `sstore_reset_refund`, and `selfdestruct_refund`.
///
/// # Gas forwarding: no 63/64 retention (verified fact #5)
///
/// Ethereum's EIP-150 retains 1/64 of the caller's gas on every inner CALL and
/// CREATE. Tron does **not**: `Program.getCallEnergy` / `getCreateEnergy`
/// (`Program.java:1834-1848`) apply the 1/64 cut only under
/// `allowTvmCompatibleEvm && contractVersion == 1`, a flag that is off on both
/// mainnet and Nile (probed 2026-07-12); the default path forwards
/// `min(requested, all-available)`. revm applies the cut unconditionally in the
/// call/create instructions (`call_helpers.rs:92-96`, `contract.rs:101-110`),
/// reading the reduction divisor from `GasId::call_stipend_reduction` (64 in the
/// FRONTIER `GasParams`, gated on the CANCUN runtime spec). Setting that divisor
/// to `u64::MAX` makes `gas_limit - gas_limit / divisor == gas_limit` for every
/// realistic gas limit, i.e. the full remaining gas is forwarded — matching
/// Tron. Without this an inner frame would see 63/64 of the parent's gas, so
/// `gasleft()` and nested out-of-gas boundaries would diverge from on-chain.
///
/// # Known limitation: call depth
///
/// Tron caps call depth at 64 (`Program.java` `MAX_DEPTH=64`) versus revm's
/// 1024. revm 41 hard-codes this as `revm_primitives::constants::CALL_STACK_LIMIT`
/// (a `pub const`, not a config field) and reads it directly inside the default
/// handler's frame construction (`revm-handler` `frame.rs:175,287`). Overriding
/// it would require reimplementing `make_call_frame`/`make_create_frame`, which
/// is out of scope for the energy model. Contracts that recurse past depth 64
/// therefore succeed locally but would revert on-chain. Recorded as a known
/// limitation in `docs/tron/STATUS.md`.
pub(super) fn apply_tron_energy<DB: Database, I>(
    inner: &mut RevmEvm<
        EthEvmContext<DB>,
        I,
        EthInstructions<EthInterpreter, EthEvmContext<DB>>,
        PrecompilesMap,
        EthFrame,
    >,
) {
    // 1. Static gas table: wholesale FRONTIER baseline (pre-EIP-150 = Tron
    // energy tiers), then re-assert the TVM static deltas on top.
    let instructions = &mut inner.instruction;
    *instructions.gas_table_mut() = gas_table();
    instructions.insert_gas(opcode::TLOAD, TRON_TRANSIENT_ENERGY);
    instructions.insert_gas(opcode::TSTORE, TRON_TRANSIENT_ENERGY);
    instructions.insert_gas(opcode::SELFDESTRUCT, TRON_SELFDESTRUCT_ENERGY);

    // 2. Dynamic gas: FRONTIER GasParams + the Tron-specific overrides.
    inner.ctx.cfg.set_gas_params(tron_gas_params());

    // 3. Limits Tron does not have: no EIP-170 (24KB code cap) and no EIP-3860
    // size cap. `Program.java:927-943` has no size check. The per-word initcode
    // metering EIP-3860 also adds (`EnergyCost:414-419` `getCreateCost` has no
    // per-word term) is disabled in `tron_gas_params` via `initcode_per_word`,
    // because `limit_contract_initcode_size` only lifts the size cap, not the
    // metering.
    inner.ctx.cfg.limit_contract_code_size = Some(usize::MAX);
    inner.ctx.cfg.limit_contract_initcode_size = Some(usize::MAX);
}

/// Builds the Tron energy [`GasParams`]: the FRONTIER baseline plus the
/// Tron-specific dynamic overrides. Extracted from [`apply_tron_energy`] so the
/// exact override values can be unit-tested directly, independent of the full
/// EVM wiring.
pub(super) fn tron_gas_params() -> GasParams {
    let mut gas_params = GasParams::new_spec(SpecId::FRONTIER);
    gas_params.override_gas([
        // EXP byte energy = 10 (Tron `EXP_BYTE_ENERGY`). Already 10 at
        // FRONTIER, set explicitly so a spec drift cannot re-inflate it to
        // the post-EIP-160 50.
        (GasId::exp_byte_gas(), 10),
        // Tron has no gas-refund counter (see doc comment), so every refund
        // source is zeroed. These are refund-only GasIds distinct from the
        // SSTORE *cost* GasIds, so zeroing them does not change any cost.
        // The Istanbul net-metering path awards `sstore_set_refund` /
        // `sstore_reset_refund` for in-tx 0->x->0 (and x->y->x) storage
        // restorations, not just `sstore_clearing_slot_refund`; all three
        // plus `selfdestruct_refund` must be zeroed or a clearing/restoring
        // SSTORE would be discounted (EIP-3529 caps it at gas_used/5).
        (GasId::sstore_clearing_slot_refund(), 0), // FRONTIER: 15000.
        (GasId::sstore_set_refund(), 0),           // FRONTIER: 15000.
        (GasId::sstore_reset_refund(), 0),         // FRONTIER: 0, but set post-Istanbul.
        (GasId::selfdestruct_refund(), 0),         // FRONTIER: 24000.
        // SUICIDE_V2 dead-account topup = NEW_ACCT_CALL. FRONTIER: 0.
        (GasId::new_account_cost_for_selfdestruct(), TRON_SELFDESTRUCT_NEW_ACCOUNT_ENERGY),
        // Intrinsic tx gas is Tron *bandwidth*, not energy. Zero the 21000
        // base stipend and the per-token calldata cost so `energy_used`
        // excludes the intrinsic (bandwidth is metered separately from the
        // protobuf tx envelope).
        (GasId::tx_base_stipend(), 0),
        (GasId::tx_token_cost(), 0),
        // No EIP-3860 per-word initcode metering. `initcode_per_word` is 2 for
        // *every* spec in the base table (FRONTIER included) and is charged by
        // the inner CREATE / stock CREATE2 instructions
        // (`contract.rs:44-58`, gated on the CANCUN runtime spec), so lifting
        // `limit_contract_initcode_size` alone leaves the 2/word charge. Tron
        // `EnergyCost.getCreateCost` (:414-419) has no per-word term, so zero
        // it: an inner CREATE of an N-word initcode otherwise overcharges 2N.
        (GasId::initcode_per_word(), 0),
        // No EIP-150 63/64 gas retention on inner CALL/CREATE (see doc comment,
        // verified fact #5). `call_stipend_reduction(gas_limit)` computes
        // `gas_limit - gas_limit / divisor`; with divisor `u64::MAX` the second
        // term is 0 for every realistic gas limit, so the full gas is forwarded.
        (GasId::call_stipend_reduction(), u64::MAX),
    ]);
    gas_params
}

#[cfg(test)]
mod tests {
    use super::{TRON_ENERGY_FEE_SUN, tron_gas_params};
    use crate::evm::tron::TronEvmFactory;
    use alloy_evm::{EthEvmFactory, Evm, EvmEnv, EvmFactory};
    use alloy_primitives::{B256, TxKind, U256, hex};
    use revm::{
        context::{CfgEnv, TxEnv},
        context_interface::cfg::GasParams,
        database::{CacheDB, EmptyDB},
        primitives::hardfork::SpecId,
    };

    /// A CANCUN environment mirroring the tron sandbox (`evm_version = "cancun"`).
    /// A non-zero block gas limit and a `Some` prevrandao keep the post-merge
    /// validation and the Ethereum contrast opcodes well-defined.
    fn cancun_env() -> EvmEnv {
        let mut env: EvmEnv<SpecId> =
            EvmEnv { cfg_env: CfgEnv::new_with_spec(SpecId::CANCUN), ..Default::default() };
        env.block_env.gas_limit = 30_000_000;
        env.block_env.prevrandao = Some(B256::with_last_byte(0x11));
        env
    }

    fn create_tx(bytecode: Vec<u8>) -> TxEnv {
        TxEnv {
            kind: TxKind::Create,
            data: bytecode.into(),
            gas_limit: 10_000_000,
            ..Default::default()
        }
    }

    /// Runs `code` as a top-level creation tx on the Tron factory and returns the
    /// consumed energy. Asserts success so a halt does not masquerade as a value.
    fn tron_energy(code: &[u8]) -> u64 {
        let mut evm = TronEvmFactory.create_evm(CacheDB::<EmptyDB>::default(), cancun_env());
        let out = evm.transact_raw(create_tx(code.to_vec())).unwrap();
        assert!(out.result.is_success(), "tron exec must succeed: {:?}", out.result);
        out.result.tx_gas_used()
    }

    /// Same, on the vanilla Ethereum factory (for non-tautology contrast).
    fn eth_energy(code: &[u8]) -> u64 {
        let mut evm =
            EthEvmFactory::default().create_evm(CacheDB::<EmptyDB>::default(), cancun_env());
        let out = evm.transact_raw(create_tx(code.to_vec())).unwrap();
        assert!(out.result.is_success(), "eth exec must succeed: {:?}", out.result);
        out.result.tx_gas_used()
    }

    /// Runs `code` as a creation tx on the Tron factory and returns the deployed
    /// runtime bytes (i.e. what the constructor RETURNed), as a big-endian word.
    fn tron_returned_word(code: &[u8]) -> U256 {
        let mut evm = TronEvmFactory.create_evm(CacheDB::<EmptyDB>::default(), cancun_env());
        let out = evm.transact_raw(create_tx(code.to_vec())).unwrap();
        assert!(out.result.is_success(), "tron exec must succeed: {:?}", out.result);
        U256::from_be_slice(out.result.output().unwrap())
    }

    fn eth_returned_word(code: &[u8]) -> U256 {
        let mut evm =
            EthEvmFactory::default().create_evm(CacheDB::<EmptyDB>::default(), cancun_env());
        let out = evm.transact_raw(create_tx(code.to_vec())).unwrap();
        assert!(out.result.is_success(), "eth exec must succeed: {:?}", out.result);
        U256::from_be_slice(out.result.output().unwrap())
    }

    // ---- per-opcode energy vectors (hand-computed from EnergyCost.java) ----
    //
    // Each snippet runs as a creation tx; the Tron intrinsic is zero (bandwidth,
    // not energy), so `gas_used` is exactly the sum of the executed opcodes'
    // energy plus 200/byte for any deployed runtime. Every snippet ends in
    // `5f 5f f3` (PUSH0 size, PUSH0 offset, RETURN) which returns empty runtime
    // (cost 2+2+0, no code deposit) unless noted.

    #[test]
    fn sload_energy_is_50_not_2100() {
        // 5f PUSH0(2) | 54 SLOAD(50) | 50 POP(2) | 5f 5f f3 (2+2+0)
        let code = hex::decode("5f5450 5f5ff3".replace(' ', "")).unwrap();
        assert_eq!(
            tron_energy(&code),
            2 + 50 + 2 + 2 + 2,
            "cold SLOAD is 50 on Tron (no EIP-2929)"
        );
        assert!(
            eth_energy(&code) > tron_energy(&code),
            "Ethereum must differ (EIP-2929 + intrinsic)"
        );
    }

    #[test]
    fn balance_energy_is_20_not_2600() {
        // 30 ADDRESS(2) | 31 BALANCE(20) | 50 POP(2) | 5f 5f f3 (2+2+0)
        let code = hex::decode("303150 5f5ff3".replace(' ', "")).unwrap();
        assert_eq!(tron_energy(&code), 2 + 20 + 2 + 2 + 2, "BALANCE is 20 on Tron (no EIP-2929)");
        assert!(eth_energy(&code) > tron_energy(&code), "Ethereum must differ");
    }

    #[test]
    fn exp_one_byte_exponent_is_10_plus_10() {
        // 60 ff PUSH1 exp(3) | 60 02 PUSH1 base(3) | 0a EXP(10+10) | 50 POP(2) | 5f 5f f3
        let code = hex::decode("60ff60020a50 5f5ff3".replace(' ', "")).unwrap();
        assert_eq!(
            tron_energy(&code),
            3 + 3 + 20 + 2 + 2 + 2,
            "EXP byte energy is 10 on Tron (not 50)"
        );
        assert!(
            eth_energy(&code) != tron_energy(&code),
            "Ethereum must differ (EXP byte 50 + intrinsic)"
        );
    }

    #[test]
    fn sstore_set_zero_to_nonzero_is_20000() {
        // 60 01 PUSH1 1 value(3) | 5f PUSH0 key(2) | 55 SSTORE set(20000) | 5f 5f f3
        let code = hex::decode("60015f55 5f5ff3".replace(' ', "")).unwrap();
        assert_eq!(tron_energy(&code), 3 + 2 + 20_000 + 2 + 2, "SSTORE 0->x SET is 20000");
        assert!(
            eth_energy(&code) != tron_energy(&code),
            "Ethereum must differ (EIP-2200/2929 + intrinsic)"
        );
    }

    #[test]
    fn sstore_clear_nonzero_to_zero_is_5000_with_no_refund() {
        // set 0->1 (20000) then clear 1->0 (5000): the FRONTIER 15000 clearing
        // refund is zeroed, so gas_used is the full undiscounted sum. If a refund
        // were applied, gas_used would be 15000 lower.
        // 60 01(3) 5f(2) 55 set(20000) | 5f(2) 5f(2) 55 clear(5000) | 5f 5f f3 (2+2+0)
        let code = hex::decode("60015f55 5f5f55 5f5ff3".replace(' ', "")).unwrap();
        let expected = 3 + 2 + 20_000 + 2 + 2 + 5_000 + 2 + 2;
        assert_eq!(tron_energy(&code), expected, "SSTORE x->0 CLEAR is 5000 and accrues NO refund");
    }

    #[test]
    fn tload_tstore_are_100_each() {
        // 5f(2) 5f(2) 5d TSTORE(100) | 5f(2) 5c TLOAD(100) | 50 POP(2) | 5f 5f f3
        let code = hex::decode("5f5f5d 5f5c50 5f5ff3".replace(' ', "")).unwrap();
        assert_eq!(
            tron_energy(&code),
            2 + 2 + 100 + 2 + 100 + 2 + 2 + 2,
            "TLOAD/TSTORE are 100 each"
        );
        assert!(eth_energy(&code) != tron_energy(&code), "Ethereum must differ (intrinsic)");
    }

    #[test]
    fn keccak256_32_bytes_is_30_plus_6_plus_memory() {
        // 60 20 PUSH1 32 size(3) | 5f PUSH0 offset(2) | 20 KECCAK256 = 30 base +
        // 6/word*1 + memory-expand-to-1-word(3) | 50 POP(2) | 5f 5f f3
        let code = hex::decode("60205f2050 5f5ff3".replace(' ', "")).unwrap();
        assert_eq!(
            tron_energy(&code),
            3 + 2 + (30 + 6 + 3) + 2 + 2 + 2,
            "KECCAK256 = 30 + 6/word + memory"
        );
        assert!(eth_energy(&code) != tron_energy(&code), "Ethereum must differ (intrinsic)");
    }

    #[test]
    fn deploy_price_is_execution_plus_200_per_runtime_byte() {
        // Constructor writes one byte and RETURNs it as the 1-byte runtime.
        // 60 01(3) 60 00(3) 53 MSTORE8 = 3 + mem-expand-1-word(3) | 60 01(3)
        // 60 00(3) f3 RETURN(0) -> execution 18; code deposit 200*1 (CREATE_DATA),
        // and NO EIP-3860 initcode metering and NO 21000/32000 intrinsic.
        let code = hex::decode("600160005360016000f3").unwrap();
        assert_eq!(tron_energy(&code), 18 + 200, "deploy energy = execution + 200/runtime-byte");
        // Ethereum adds 21000 + 32000 intrinsic + initcode metering.
        assert!(eth_energy(&code) > 50_000, "Ethereum deploy carries the create intrinsic");
    }

    // ---- block/tx-op override semantics ----

    #[test]
    fn difficulty_and_gaslimit_are_forced_to_zero() {
        // 44 DIFFICULTY | 5f 52 MSTORE | 60 20 5f f3 RETURN 32 bytes
        let difficulty = hex::decode("445f5260205ff3").unwrap();
        // 45 GASLIMIT | ...
        let gaslimit = hex::decode("455f5260205ff3").unwrap();
        assert_eq!(tron_returned_word(&difficulty), U256::ZERO, "Tron DIFFICULTY is always 0");
        assert_eq!(tron_returned_word(&gaslimit), U256::ZERO, "Tron GASLIMIT is always 0");
        // Ethereum returns the live env values (prevrandao / block gas limit).
        assert_ne!(
            eth_returned_word(&difficulty),
            U256::ZERO,
            "Ethereum DIFFICULTY returns prevrandao"
        );
        assert_eq!(
            eth_returned_word(&gaslimit),
            U256::from(30_000_000u64),
            "Ethereum GASLIMIT returns block gas limit"
        );
    }

    #[test]
    fn basefee_returns_tron_energy_fee() {
        // 48 BASEFEE | 5f 52 MSTORE | 60 20 5f f3 RETURN 32 bytes
        let basefee = hex::decode("485f5260205ff3").unwrap();
        assert_eq!(
            tron_returned_word(&basefee),
            U256::from(TRON_ENERGY_FEE_SUN),
            "Tron BASEFEE is getEnergyFee()=100"
        );
        // Ethereum returns the block base fee (0 in this env), not 100.
        assert_eq!(
            eth_returned_word(&basefee),
            U256::ZERO,
            "Ethereum BASEFEE returns the block base fee"
        );
    }

    // ---- inner CREATE/CREATE2 gas: no EIP-3860 metering, no 63/64 retention ----

    /// The Tron `GasParams` charge no per-word EIP-3860 initcode metering,
    /// unlike the FRONTIER base table (which carries 2/word for every spec).
    #[test]
    fn tron_gas_params_disable_initcode_metering() {
        let tron = tron_gas_params();
        // 64 bytes = 2 words; FRONTIER would charge 2 * 2 = 4, Tron charges 0.
        assert_eq!(tron.initcode_cost(64), 0, "Tron has no per-word initcode metering");
        assert_eq!(
            GasParams::new_spec(SpecId::FRONTIER).initcode_cost(64),
            4,
            "FRONTIER base meters 2/word (the value Tron must override away)"
        );
    }

    /// The Tron `GasParams` forward the full gas to inner CALL/CREATE frames,
    /// unlike FRONTIER which retains 1/64 (EIP-150).
    #[test]
    fn tron_gas_params_forward_all_gas_no_63_64() {
        let tron = tron_gas_params();
        assert_eq!(
            tron.call_stipend_reduction(9_967_982),
            9_967_982,
            "Tron forwards all gas (no 1/64 retention)"
        );
        assert_eq!(
            GasParams::new_spec(SpecId::FRONTIER).call_stipend_reduction(9_967_982),
            9_967_982 - 9_967_982 / 64,
            "FRONTIER retains 1/64 (the behaviour Tron must override away)"
        );
    }

    /// An inner CREATE of a 32-byte initcode costs execution + CREATE(32000) with
    /// **no** per-word EIP-3860 term. The snippet stores a 32-byte child initcode
    /// (`5f 5f f3` = return empty, then padding), CREATEs it, POPs the address and
    /// returns empty. Hand-computed against `EnergyCost.getCreateCost`: setup is
    /// `PUSH32 3, PUSH0 2, MSTORE 3+mem 3, PUSH1 3, PUSH0 2, PUSH0 2` = 18; then
    /// `CREATE 32000`, the child `PUSH0 2, PUSH0 2, RETURN 0` = 4, and the tail
    /// `POP 2, PUSH0 2, PUSH0 2, RETURN 0` = 6, totalling 32028. With EIP-3860
    /// metering left on it would be 32030 (2/word extra).
    #[test]
    fn inner_create_has_no_eip3860_metering() {
        let mut child = vec![0x5f, 0x5f, 0xf3];
        child.resize(32, 0x00);
        let mut code = vec![0x7f]; // PUSH32 child-initcode word.
        code.extend_from_slice(&child);
        code.extend_from_slice(&[0x5f, 0x52]); // PUSH0 offset, MSTORE.
        code.extend_from_slice(&[0x60, 0x20, 0x5f, 0x5f, 0xf0]); // PUSH1 32, PUSH0, PUSH0, CREATE.
        code.extend_from_slice(&[0x50, 0x5f, 0x5f, 0xf3]); // POP, PUSH0, PUSH0, RETURN.

        assert_eq!(
            tron_energy(&code),
            32_028,
            "inner CREATE of a 32-byte initcode is 32028 (no 2/word EIP-3860 term)"
        );
        assert_ne!(
            eth_energy(&code),
            tron_energy(&code),
            "Ethereum must differ (intrinsic + EIP-3860 metering)"
        );
    }

    /// An inner CREATE forwards the full remaining gas to the child (no 63/64
    /// retention). The child initcode reads `GAS` and REVERTs with it, so the
    /// parent surfaces the child's forwarded budget via RETURNDATACOPY.
    /// Hand-computed: parent spends `PUSH32 3, PUSH0 2, MSTORE 3+mem 3, PUSH1 3,
    /// PUSH0 2, PUSH0 2` = 18 before CREATE, then CREATE deducts 32000, so the
    /// child budget is `10_000_000 - 18 - 32000 = 9_967_982`; the child `GAS` op
    /// charges 2 and reports 9_967_980. With the 63/64 rule left on it would
    /// report 9_812_229.
    #[test]
    fn inner_create_forwards_all_gas() {
        // child = GAS, PUSH0, MSTORE, PUSH1 32, PUSH0, REVERT (reverts with gasleft).
        let child = [0x5a, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xfd];
        let mut word = [0u8; 32];
        word[..child.len()].copy_from_slice(&child);
        let mut code = vec![0x7f]; // PUSH32 child-initcode word (child in the top 7 bytes).
        code.extend_from_slice(&word);
        code.extend_from_slice(&[0x5f, 0x52]); // PUSH0 offset, MSTORE.
        code.extend_from_slice(&[0x60, 0x07, 0x5f, 0x5f, 0xf0]); // PUSH1 7, PUSH0, PUSH0, CREATE.
        code.push(0x50); // POP the (zero) create result.
        // RETURNDATACOPY(dest=0, off=0, size=32); RETURN(0, 32).
        code.extend_from_slice(&[0x60, 0x20, 0x5f, 0x5f, 0x3e]); // PUSH1 32, PUSH0, PUSH0, RETURNDATACOPY.
        code.extend_from_slice(&[0x60, 0x20, 0x5f, 0xf3]); // PUSH1 32, PUSH0, RETURN.

        assert_eq!(
            tron_returned_word(&code),
            U256::from(9_967_980u64),
            "child sees the full forwarded gas (no 63/64 retention)"
        );
        // Ethereum retains 1/64 and carries a different create intrinsic, so the
        // child observes a strictly smaller, different budget.
        assert_ne!(
            eth_returned_word(&code),
            tron_returned_word(&code),
            "Ethereum must differ (63/64 retention + create intrinsic)"
        );
    }

    #[test]
    fn gasprice_blobhash_blobbasefee_are_forced_to_zero() {
        // 3a GASPRICE
        let gasprice = hex::decode("3a5f5260205ff3").unwrap();
        // 5f 49 BLOBHASH(index 0)
        let blobhash = hex::decode("5f495f5260205ff3").unwrap();
        // 4a BLOBBASEFEE
        let blobbasefee = hex::decode("4a5f5260205ff3").unwrap();
        assert_eq!(
            tron_returned_word(&gasprice),
            U256::ZERO,
            "Tron GASPRICE is 0 (CompatibleEvm off)"
        );
        assert_eq!(tron_returned_word(&blobhash), U256::ZERO, "Tron BLOBHASH is stubbed to 0");
        assert_eq!(
            tron_returned_word(&blobbasefee),
            U256::ZERO,
            "Tron BLOBBASEFEE is stubbed to 0"
        );
    }
}
