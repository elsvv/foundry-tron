//! Tron CREATE2 (0xF5) address scheme.
//!
//! Tron computes the CREATE2 address as
//! `keccak256(0x41 ‖ sender ‖ salt ‖ keccak256(init_code))[12..]`
//! (`WalletUtil.generateContractAddress2` / `Program.createContract2` @develop),
//! which diverges from Ethereum's EIP-1014 on two points: there is **no** `0xff`
//! domain-separator byte, and the sender is hashed as the 21-byte 0x41-prefixed
//! Tron address rather than the 20-byte EVM address. The formula itself lives in
//! [`foundry_tron_primitives::address::create2_address`] so both this
//! instruction and the `computeCreate2Address` cheatcode reuse one source.
//!
//! revm 41 already exposes the seam we need: [`CreateScheme::Custom`] makes
//! `CreateInputs::created_address` return a caller-supplied address verbatim,
//! honored by both the create frame and Foundry's cheatcode inspector. So the
//! only work is a custom 0xF5 instruction that reproduces the stock
//! `create::<true>` body (`revm-interpreter` `contract.rs`), computes the Tron
//! address at instruction time, and hands revm a `Custom` scheme — guaranteeing
//! the address is fixed *before* any inspector or frame reads it.
//!
//! Two deliberate deltas from the stock body (everything else is a faithful
//! copy, including memory-expansion gas, the `create2_cost` charge, and the
//! EIP-150 forward reduction):
//!
//! 1. **No EIP-3860 initcode metering.** Tron has no initcode size cap and no per-word initcode
//!    charge (`Program.java:927-943`, `EnergyCost:414-419`). The stock body meters
//!    `initcode_cost(len)` under SHANGHAI; the Tron energy model already zeroes
//!    `GasId::initcode_per_word`, but this instruction also omits the branch structurally so a
//!    future spec bump cannot re-introduce the charge or the size-limit halt.
//! 2. **`CreateScheme::Custom` with the Tron address**, in place of the stock
//!    `CreateScheme::Create2 { salt }` (which would apply the EVM 0xff scheme).
//!
//! The `create2_cost` charge (`32000 + 6/word`, matching java-tron
//! `EnergyCost.getCreate2Cost`) and memory expansion are unchanged: the static
//! gas for 0xF5 is 0 in revm (`instructions.rs:470`) and the whole cost is
//! metered dynamically inside the instruction, so this is inserted with a static
//! tier of 0 (see [`super::inject_tron_extensions`]).

use alloy_primitives::{B256, Bytes, U256, keccak256};
use revm::{
    context_interface::{CreateScheme, Host},
    interpreter::{
        CreateInputs, FrameInput, InstructionContext, InstructionResult, InterpreterAction,
        InterpreterTypes,
        interpreter_types::{InputsTr, LoopControl, MemoryTr, RuntimeFlag, StackTr},
    },
    primitives::hardfork::SpecId,
};

pub use foundry_tron_primitives::address::create2_address as tron_create2_address;

/// Converts a `U256` stack word to a `usize`, failing the instruction with
/// `InvalidOperandOOG` when it does not fit — mirrors revm's `as_usize_or_fail!`.
const fn as_usize_or_fail(v: U256) -> Result<usize, InstructionResult> {
    let x = v.as_limbs();
    if x[1] != 0 || x[2] != 0 || x[3] != 0 || x[0] > usize::MAX as u64 {
        return Err(InstructionResult::InvalidOperandOOG);
    }
    Ok(x[0] as usize)
}

/// Records a regular gas cost, failing the instruction with `OutOfGas` when it
/// would exceed the remaining gas — mirrors revm's `gas!`.
const fn record_gas<W: InterpreterTypes, H: Host + ?Sized>(
    ctx: &mut InstructionContext<'_, H, W>,
    cost: u64,
) -> Result<(), InstructionResult> {
    if !ctx.interpreter.gas.record_regular_cost(cost) {
        return Err(InstructionResult::OutOfGas);
    }
    Ok(())
}

/// 0xF5 CREATE2 with the Tron address scheme. A faithful copy of revm's
/// `create::<true>` with the two deltas documented at the module level: no
/// EIP-3860 metering and a `CreateScheme::Custom` carrying the Tron address.
pub(super) fn op_create2<W: InterpreterTypes, H: Host + ?Sized>(
    mut ctx: InstructionContext<'_, H, W>,
) -> Result<(), InstructionResult> {
    // CREATE in a static context is always an error (checked before gas, like
    // the stock body).
    if ctx.interpreter.runtime_flag.is_static() {
        return Err(InstructionResult::StateChangeDuringStaticCall);
    }

    // EIP-1014: CREATE2 requires PETERSBURG. Always true on the Tron CANCUN
    // path; kept for parity with the stock body.
    if !ctx.interpreter.runtime_flag.spec_id().is_enabled_in(SpecId::PETERSBURG) {
        return Err(InstructionResult::NotActivated);
    }

    let Some([value, code_offset, len]) = ctx.interpreter.stack.popn::<3>() else {
        return Err(InstructionResult::StackUnderflow);
    };
    let len = as_usize_or_fail(len)?;

    // Delta 1: the stock body meters EIP-3860 initcode cost and enforces the
    // initcode size cap here under SHANGHAI. Tron has neither, so the whole
    // branch is omitted; only memory expansion and the raw code read remain.
    let mut code = Bytes::new();
    if len != 0 {
        let code_offset = as_usize_or_fail(code_offset)?;
        ctx.interpreter.resize_memory(ctx.host.gas_params(), code_offset, len)?;
        code = Bytes::copy_from_slice(ctx.interpreter.memory.slice_len(code_offset, len).as_ref());
    }

    let Some([salt]) = ctx.interpreter.stack.popn::<1>() else {
        return Err(InstructionResult::StackUnderflow);
    };
    // CREATE2 dynamic gas: `create() + keccak256_per_word * num_words(len)` =
    // 32000 + 6/word under the Tron FRONTIER gas params (java-tron
    // `EnergyCost.getCreate2Cost`).
    let create2_cost = ctx.host.gas_params().create2_cost(len);
    record_gas(&mut ctx, create2_cost)?;

    // EIP-8037 (Amsterdam) state gas. Not active on Tron, so this is a no-op;
    // retained for structural parity with the stock body.
    if ctx.host.is_amsterdam_eip8037_enabled() {
        let state_gas = ctx.host.gas_params().create_state_gas();
        if !ctx.interpreter.gas.record_state_cost(state_gas) {
            return Err(InstructionResult::OutOfGas);
        }
    }

    // EIP-150 forward reduction. Under the Tron energy model the
    // `call_stipend_reduction` divisor is `u64::MAX`, so this forwards the full
    // remaining gas (no 63/64 retention); the branch and charge stay for parity.
    let mut gas_limit = ctx.interpreter.gas.remaining();
    if ctx.interpreter.runtime_flag.spec_id().is_enabled_in(SpecId::TANGERINE) {
        gas_limit = ctx.host.gas_params().call_stipend_reduction(gas_limit);
    }
    record_gas(&mut ctx, gas_limit)?;

    // Delta 2: compute the Tron CREATE2 address now and pin it via
    // `CreateScheme::Custom`. The sender is the executing contract
    // (`getContextAddress()` on Tron); the init-code hash is `keccak256(code)`.
    let sender = ctx.interpreter.input.target_address();
    let addr = tron_create2_address(sender, B256::from(salt), keccak256(code.as_ref()));

    let reservoir = ctx.interpreter.gas.reservoir();
    let create_inputs = CreateInputs::new(
        sender,
        CreateScheme::Custom { address: addr },
        value,
        code,
        gas_limit,
        reservoir,
    );
    ctx.interpreter
        .bytecode
        .set_action(InterpreterAction::NewFrame(FrameInput::Create(Box::new(create_inputs))));
    Err(InstructionResult::Suspend)
}

#[cfg(test)]
mod tests {
    use crate::evm::tron::TronEvmFactory;
    use alloy_evm::{EthEvmFactory, Evm, EvmEnv, EvmFactory};
    use alloy_primitives::{Address, B256, U256, hex, keccak256};
    use foundry_tron_primitives::address::create2_address;
    use revm::{
        context::{BlockEnv, TxEnv},
        database::{CacheDB, EmptyDB},
        primitives::{TxKind, hardfork::SpecId},
    };

    /// A constructor that CREATE2-deploys the 3-byte child initcode `5f5ff3`
    /// (PUSH0 PUSH0 RETURN → empty runtime) with salt 0x2a, then RETURNs the
    /// resulting child address as a 32-byte word. Stack layout for CREATE2 is
    /// value(top), offset, length, salt(bottom).
    ///
    /// `62 5f5ff3`(PUSH3 initcode) `5f 52`(MSTORE at 0 → initcode at mem[29..32])
    /// `60 2a`(salt) `60 03`(len) `60 1d`(offset 29) `5f`(value 0) `f5`(CREATE2)
    /// `5f 52`(MSTORE addr at 0) `60 20 5f f3`(RETURN 32 bytes).
    fn create2_probe() -> Vec<u8> {
        hex::decode("625f5ff35f52602a6003601d5ff55f5260205ff3").unwrap()
    }

    const CHILD_INITCODE: &[u8] = &[0x5f, 0x5f, 0xf3];
    const SALT: u64 = 0x2a;

    fn create_tx(bytecode: Vec<u8>) -> TxEnv {
        TxEnv {
            kind: TxKind::Create,
            data: bytecode.into(),
            gas_limit: 10_000_000,
            ..Default::default()
        }
    }

    /// Runs the probe on `factory`, returning (factory address, child address the
    /// CREATE2 produced).
    fn run<F>(factory: F) -> (Address, Address)
    where
        F: EvmFactory<Tx = TxEnv, Spec = SpecId, BlockEnv = BlockEnv>,
    {
        let mut evm = factory.create_evm(CacheDB::<EmptyDB>::default(), EvmEnv::default());
        let out = evm.transact_raw(create_tx(create2_probe())).unwrap();
        assert!(out.result.is_success(), "probe must deploy: {:?}", out.result);
        let factory_addr = out.result.created_address().expect("factory address");
        let word = out.result.output().expect("returned child address word");
        (factory_addr, Address::from_slice(&word[12..]))
    }

    #[test]
    fn create2_uses_tron_scheme_not_evm() {
        let salt = B256::from(U256::from(SALT));
        let init_code_hash = keccak256(CHILD_INITCODE);

        let (factory, child) = run(TronEvmFactory);
        // The 0xF5 override must place the child at the Tron address.
        assert_eq!(
            child,
            create2_address(factory, salt, init_code_hash),
            "Tron CREATE2 must deploy at keccak256(0x41 ‖ sender ‖ salt ‖ hash)[12..]"
        );
        // And that address must NOT be the EVM 0xff address (proves override).
        assert_ne!(
            child,
            factory.create2(salt, init_code_hash),
            "Tron CREATE2 must diverge from the EVM 0xff scheme"
        );
    }

    #[test]
    fn vanilla_create2_stays_evm_scheme() {
        let salt = B256::from(U256::from(SALT));
        let init_code_hash = keccak256(CHILD_INITCODE);

        let (factory, child) = run(EthEvmFactory::default());
        // Contrast: the stock EVM deploys at the EIP-1014 0xff address, so the
        // Tron assertion above is non-tautological.
        assert_eq!(
            child,
            factory.create2(salt, init_code_hash),
            "vanilla CREATE2 stays on the EVM 0xff scheme"
        );
        assert_ne!(child, create2_address(factory, salt, init_code_hash));
    }
}
