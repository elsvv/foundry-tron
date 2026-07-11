//! Tron EVM factory: vanilla revm + TVM opcodes 0xD0-0xD4.
//!
//! tron-solc injects a TRC-10 guard (0xD3 CALLTOKENID / 0xD2 CALLTOKENVALUE)
//! into every non-payable entry, so executing tron-solc bytecode requires
//! these instructions. Stage 1.5 stubs: no TRC-10 exists in local tests,
//! so token value/id/balance are 0; ISCONTRACT is implemented faithfully;
//! CALLTOKEN halts loudly (NotActivated) instead of silently lying.
//! Costs mirror java-tron EnergyCost.java (BASE_TIER=2, BALANCE=20, CALL=40).

use alloy_evm::{
    Database, EthEvm, EthEvmFactory, Evm, EvmEnv, EvmFactory, eth::EthEvmContext,
    precompiles::PrecompilesMap,
};
use alloy_primitives::U256;
use foundry_fork_db::DatabaseError;
use revm::{
    Inspector,
    context::{
        BlockEnv, ContextTr, DBErrorMarker, Evm as RevmEvm, LocalContextTr, TxEnv,
        result::{EVMError, HaltReason, ResultAndState},
    },
    context_interface::Host,
    handler::{
        EthFrame, EvmTr, FrameResult, Handler, MainnetHandler, instructions::EthInstructions,
    },
    inspector::{InspectorHandler, NoOpInspector},
    interpreter::{
        FrameInput, Instruction, InstructionContext, InstructionResult, InterpreterTypes,
        SharedMemory, interpreter::EthInterpreter, interpreter_action::FrameInit,
        interpreter_types::StackTr,
    },
    primitives::hardfork::SpecId,
};

use crate::{
    FoundryContextExt, FoundryInspectorExt,
    backend::{DatabaseExt, JournaledState},
    evm::{FoundryEvmFactory, NestedEvm},
};

/// EVM factory that extends vanilla revm with the TVM opcodes 0xD0-0xD4.
#[derive(Clone, Copy, Debug, Default)]
pub struct TronEvmFactory;

impl EvmFactory for TronEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>>> = EthEvm<DB, I, PrecompilesMap>;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Tx = TxEnv;
    type Error<E: DBErrorMarker> = EVMError<E>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        inject_tron_opcodes(EthEvmFactory::default().create_evm(db, env), false)
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        inject_tron_opcodes(
            EthEvmFactory::default().create_evm_with_inspector(db, env, inspector),
            true,
        )
    }
}

/// Overwrites the TVM opcodes 0xD0-0xD4 in the instruction table of a freshly
/// built [`EthEvm`] and rebuilds it, preserving the `inspect` flag.
fn inject_tron_opcodes<DB: Database, I: Inspector<EthEvmContext<DB>>>(
    evm: EthEvm<DB, I, PrecompilesMap>,
    inspect: bool,
) -> EthEvm<DB, I, PrecompilesMap> {
    let mut inner = evm.into_inner();
    let table = &mut inner.instruction;
    table.insert_instruction(0xD0, Instruction::new(op_calltoken), 40);
    table.insert_instruction(0xD1, Instruction::new(op_tokenbalance), 20);
    table.insert_instruction(0xD2, Instruction::new(op_calltokenvalue), 2);
    table.insert_instruction(0xD3, Instruction::new(op_calltokenid), 2);
    table.insert_instruction(0xD4, Instruction::new(op_iscontract), 20);
    EthEvm::new(inner, inspect)
}

/// Pops `pops` values off the stack and pushes a single zero word.
fn push_zero<W: InterpreterTypes, H: Host + ?Sized>(
    ctx: InstructionContext<'_, H, W>,
    pops: usize,
) -> Result<(), InstructionResult> {
    for _ in 0..pops {
        if ctx.interpreter.stack.pop().is_none() {
            return Err(InstructionResult::StackUnderflow);
        }
    }
    if !ctx.interpreter.stack.push(U256::ZERO) {
        return Err(InstructionResult::StackOverflow);
    }
    Ok(())
}

/// 0xD2 CALLTOKENVALUE: no TRC-10 in local tests -> 0 (java-tron default).
fn op_calltokenvalue<W: InterpreterTypes, H: Host + ?Sized>(
    ctx: InstructionContext<'_, H, W>,
) -> Result<(), InstructionResult> {
    push_zero(ctx, 0)
}

/// 0xD3 CALLTOKENID: no TRC-10 -> 0.
fn op_calltokenid<W: InterpreterTypes, H: Host + ?Sized>(
    ctx: InstructionContext<'_, H, W>,
) -> Result<(), InstructionResult> {
    push_zero(ctx, 0)
}

/// 0xD1 TOKENBALANCE (tokenId, address -> balance): no TRC-10 -> 0.
fn op_tokenbalance<W: InterpreterTypes, H: Host + ?Sized>(
    ctx: InstructionContext<'_, H, W>,
) -> Result<(), InstructionResult> {
    push_zero(ctx, 2)
}

/// 0xD0 CALLTOKEN (8 args -> success): TRC-10 transfers are out of scope --
/// halt loudly rather than fake success.
fn op_calltoken<W: InterpreterTypes, H: Host + ?Sized>(
    ctx: InstructionContext<'_, H, W>,
) -> Result<(), InstructionResult> {
    for _ in 0..8 {
        if ctx.interpreter.stack.pop().is_none() {
            return Err(InstructionResult::StackUnderflow);
        }
    }
    Err(InstructionResult::NotActivated)
}

/// 0xD4 ISCONTRACT (address -> bool): faithful -- true iff code is non-empty (TIP-44).
fn op_iscontract<W: InterpreterTypes, H: Host + ?Sized>(
    ctx: InstructionContext<'_, H, W>,
) -> Result<(), InstructionResult> {
    let Some(addr) = ctx.interpreter.stack.pop_address() else {
        return Err(InstructionResult::StackUnderflow);
    };
    let is_contract = ctx.host.load_account_code(addr).map(|c| !c.data.is_empty()).unwrap_or(false);
    if !ctx.interpreter.stack.push(U256::from(is_contract as u64)) {
        return Err(InstructionResult::StackOverflow);
    }
    Ok(())
}

type TronCtx<'db> = EthEvmContext<&'db mut dyn DatabaseExt<TronEvmFactory>>;

type TronEvmHandler<'db, I> =
    MainnetHandler<TronRevmEvm<'db, I>, EVMError<DatabaseError>, EthFrame>;

pub type TronRevmEvm<'db, I> = RevmEvm<
    TronCtx<'db>,
    I,
    EthInstructions<EthInterpreter, TronCtx<'db>>,
    PrecompilesMap,
    EthFrame,
>;

impl FoundryEvmFactory for TronEvmFactory {
    type FoundryContext<'db> = EthEvmContext<&'db mut dyn DatabaseExt<Self>>;

    type FoundryEvm<'db, I: FoundryInspectorExt<Self::FoundryContext<'db>>> =
        EthEvm<&'db mut dyn DatabaseExt<Self>, I, Self::Precompiles>;

    fn create_foundry_evm_with_inspector<'db, I: FoundryInspectorExt<Self::FoundryContext<'db>>>(
        &self,
        db: &'db mut dyn DatabaseExt<Self>,
        evm_env: EvmEnv,
        inspector: I,
    ) -> Self::FoundryEvm<'db, I> {
        let mut tron_evm = Self.create_evm_with_inspector(db, evm_env, inspector);
        tron_evm.cfg.tx_chain_id_check = true;
        tron_evm.inspector().get_networks().inject_precompiles(tron_evm.precompiles_mut());
        tron_evm
    }

    fn create_foundry_nested_evm<'db>(
        &self,
        db: &'db mut dyn DatabaseExt<Self>,
        evm_env: EvmEnv,
        inspector: &'db mut dyn FoundryInspectorExt<Self::FoundryContext<'db>>,
    ) -> Box<dyn NestedEvm<Spec = SpecId, Block = BlockEnv, Tx = TxEnv> + 'db> {
        Box::new(self.create_foundry_evm_with_inspector(db, evm_env, inspector).into_inner())
    }
}

impl<'db, I: FoundryInspectorExt<EthEvmContext<&'db mut dyn DatabaseExt<TronEvmFactory>>>> NestedEvm
    for TronRevmEvm<'db, I>
{
    type Spec = SpecId;
    type Block = BlockEnv;
    type Tx = TxEnv;

    fn journal_inner_mut(&mut self) -> &mut JournaledState {
        &mut self.ctx_mut().journaled_state.inner
    }

    fn run_execution(&mut self, frame: FrameInput) -> Result<FrameResult, EVMError<DatabaseError>> {
        let mut handler = TronEvmHandler::<I>::default();
        let reservoir = frame.reservoir();

        // Create first frame.
        let memory =
            SharedMemory::new_with_buffer(self.ctx_ref().local().shared_memory_buffer().clone());
        let first_frame_input = FrameInit { depth: 0, memory, frame_input: frame };

        // Run execution loop.
        let mut frame_result = handler.inspect_run_exec_loop(self, first_frame_input)?;

        // Handle last frame result.
        handler.last_frame_result(self, reservoir, &mut frame_result)?;

        Ok(frame_result)
    }

    fn transact_raw(&mut self, tx: Self::Tx) -> Result<ResultAndState, EVMError<DatabaseError>> {
        self.set_tx(tx);

        let result = TronEvmHandler::<I>::default().inspect_run(self)?;

        Ok(ResultAndState::new(result, self.ctx.journaled_state.inner.state.clone()))
    }

    fn to_evm_env(&self) -> EvmEnv<Self::Spec, Self::Block> {
        self.ctx_ref().evm_clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{TxKind, hex};
    use revm::database::{CacheDB, EmptyDB};

    fn creation_bytecode() -> Vec<u8> {
        hex::decode(include_str!("../../testdata/tron_counter_creation.hex").trim()).unwrap()
    }

    fn create_tx(bytecode: Vec<u8>) -> TxEnv {
        TxEnv {
            kind: TxKind::Create,
            data: bytecode.into(),
            gas_limit: 10_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn tron_factory_executes_tron_solc_bytecode() {
        let mut evm = TronEvmFactory.create_evm(CacheDB::<EmptyDB>::default(), EvmEnv::default());
        let result = evm.transact_raw(create_tx(creation_bytecode())).unwrap();
        assert!(
            result.result.is_success(),
            "tron-solc creation bytecode must deploy on TronEvmFactory: {:?}",
            result.result
        );
        let out = result.result.output().unwrap();
        assert!(!out.is_empty(), "runtime code must be non-empty");
        // The runtime code also carries the TVM guard in its function dispatcher.
        assert!(out.contains(&0xd3), "runtime must contain 0xD3 guard");
    }

    #[test]
    fn vanilla_factory_fails_on_same_bytecode() {
        let mut evm =
            EthEvmFactory::default().create_evm(CacheDB::<EmptyDB>::default(), EvmEnv::default());
        let result = evm.transact_raw(create_tx(creation_bytecode())).unwrap();
        assert!(
            !result.result.is_success(),
            "vanilla EVM must NOT execute TVM opcodes -- otherwise the test above is tautological"
        );
    }

    #[test]
    fn iscontract_semantics() {
        // 0x30 ADDRESS, 0xD4 ISCONTRACT, 0x5f PUSH0, 0x52 MSTORE, PUSH1 32, PUSH0, 0xF3 RETURN.
        // Inside the creation frame ADDRESS is the contract being created; it has no code yet => 0.
        let code = hex::decode("30d45f5260205ff3").unwrap();
        let mut evm = TronEvmFactory.create_evm(CacheDB::<EmptyDB>::default(), EvmEnv::default());
        let result = evm.transact_raw(create_tx(code)).unwrap();
        // The RETURN from the creation frame becomes the runtime code: 32 zero bytes (U256::ZERO).
        let out = result.result.output().unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(U256::from_be_slice(out), U256::ZERO);
    }
}
