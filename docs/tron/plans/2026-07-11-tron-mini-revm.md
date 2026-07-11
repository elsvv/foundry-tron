# Mini tron-revm Implementation Plan (План C2 — закрывает гейт плана C)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `TronEvmFactory` — EthEvmFactory + кастомная таблица инструкций с опкодами 0xD0–0xD4, чтобы байткод tron-solc (с TVM-guard'ом `0xD3`/`0xD2` в каждом контракте) исполнялся в `forge test`; закрыть E2E-гейт плана C (sandbox 3/3 на настоящем tron-solc-байткоде).

**Architecture:** По разведке (scout-plan-c2): instruction-таблица revm 41 — mutable `[Instruction; 256]` c публичным `EthInstructions::insert_instruction(opcode, instr, gas)`; дефолт 0xD0–0xD4 — `control::unknown` → `OpcodeNotFound`. Новый unit-тип `TronEvmFactory` (структура — по образцу `op.rs`): оба метода `EvmFactory` делегируют в `EthEvmFactory` и прогоняют результат через `inject_tron_opcodes` (into_inner → 5×insert_instruction → EthEvm::new). Диспетчеры/Backend/Executor/читкоды не трогаем — они дженерики, перемономорфизируются после смены `TronEvmNetwork::EvmFactory`.

**Семантика заглушек (по исходникам java-tron @ develop, локальные копии — в scratchpad сессии; арность из OperationRegistry.java:463-529, стоимости из EnergyCost.java):**

| байт | опкод | заглушка этапа 1.5 | gas (insert) |
|---|---|---|---|
| 0xD0 | CALLTOKEN (8 in → 1 out) | pop 8 аргументов → `Err(InstructionResult::NotActivated)` — громкий отказ, НЕ тихий push 0 (маскировал бы реальный TRC-10 вызов) | 40 |
| 0xD1 | TOKENBALANCE (2 in → 1 out) | pop tokenId, pop addr → push 0 (TRC-10 в тестах нет) | 20 |
| 0xD2 | CALLTOKENVALUE (0 in → 1 out) | push 0 (подтверждено: без TRC-10 в вызове java-tron возвращает 0) | 2 |
| 0xD3 | CALLTOKENID (0 in → 1 out) | push 0 (аналогично) | 2 |
| 0xD4 | ISCONTRACT (1 in → 1 out) | pop addr → push (код по адресу непуст) — честная реализация через `host.load_account_code` | 20 |

(ISCONTRACT: TIP-44 заявляет 400 energy, но merged-исходник java-tron даёт getBalanceCost=20 — берём 20, соответствует мейннету. Значения gas на этом этапе косметичны: forge меряет Ethereum-gas.)

**Спека:** `docs/tron/specs/2026-07-10-foundry-tron-fork-design.md` — это вынесенная вперёд часть этапа 2 (§4.4), решение пользователя от 2026-07-11.

## Global Constraints

- Пути — относительно корня foundry-репо (github.com/elsvv/foundry-tron); ветка `tron-dev`.
- **Тулчейн:** `TC=$(dirname "$(rustup which --toolchain stable cargo)"); PATH="$TC:$PATH" cargo <...>`; nightly rustfmt напрямую (см. docs/tron/STATUS.md).
- tron-solc: `~/.foundry-tron/solc/tron-solc-0.8.27` (если нет на машине — скачать: релиз `0.8.27_Democritus_v4.8.1` репо tronprotocol/solidity, ассет `solc-macos`, chmod +x; на Apple Silicon требует Rosetta 2).
- Никаких новых зависимостей; правки существующих файлов — минимальные.
- Тесты не ослаблять, fixtures — только реальный выхлоп tron-solc; `#[ignore]` запрещён.
- Точные номера строк ниже — из разведки; при сдвиге ориентироваться на именованные структуры.

---

### Task 1: TronEvmFactory с инъекцией опкодов 0xD0–0xD4

**Files:**
- Create: `crates/evm/core/src/evm/tron.rs`
- Modify: `crates/evm/core/src/evm/mod.rs` (объявление модуля ~35-43; `TronEvmNetwork::EvmFactory` ~74-77)
- Create: `crates/evm/core/testdata/tron_counter_creation.hex` (реальный байткод tron-solc)

**Interfaces:**
- Consumes: `TronEvmNetwork` (план C), `EthEvmFactory`/`EthEvm`/`EthInstructions` (alloy-evm 0.37.1 / revm 41).
- Produces: `foundry_evm_core::evm::TronEvmFactory` — `impl alloy_evm::EvmFactory` (все associated types как у `EthEvmFactory`: Tx=TxEnv, Spec=SpecId, HaltReason=HaltReason, Precompiles=PrecompilesMap, Evm=EthEvm<DB,I,PrecompilesMap>, Context=EthEvmContext<DB>) + `impl FoundryEvmFactory` (копия eth.rs:36-62) + `impl NestedEvm for TronRevmEvm<'db,I>` (копия eth.rs:64-104 с заменой типа фабрики); `TronEvmNetwork::EvmFactory = TronEvmFactory`.

- [ ] **Step 1: Fixture — реальный байткод tron-solc**

```bash
~/.foundry-tron/solc/tron-solc-0.8.27 --bin sandbox/tron-counter/src/Counter.sol \
  | awk '/Binary:/{getline; print}' > crates/evm/core/testdata/tron_counter_creation.hex
head -c 80 crates/evm/core/testdata/tron_counter_creation.hex; echo
python3 -c "h=open('crates/evm/core/testdata/tron_counter_creation.hex').read().strip(); assert 'd3' in h[:120] and 'd2' in h[:160], 'guard opcodes not found in preamble'; print('guard present, len', len(h)//2)"
```

Expected: hex начинается с `6080604052` и содержит `d3`/`d2` в преамбуле (TVM-guard). Это доказательство, что fixture — настоящий tron-solc-выхлоп.

- [ ] **Step 2: Падающий тест**

В `crates/evm/core/src/evm/tron.rs` (внизу файла; сам файл — Step 3). Тест исполняет РЕАЛЬНЫЙ tron-solc байткод через TronEvmFactory и контрастно проверяет, что ванильная фабрика на нём падает (нетавтологичность):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::{Evm, EvmEnv, EvmFactory as _};
    use alloy_primitives::{hex, TxKind, U256};
    use revm::{
        context::TxEnv,
        database::{CacheDB, EmptyDB},
    };

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
        let mut evm = TronEvmFactory::default()
            .create_evm(CacheDB::<EmptyDB>::default(), EvmEnv::default());
        let result = evm.transact_raw(create_tx(creation_bytecode())).unwrap();
        assert!(
            result.result.is_success(),
            "tron-solc creation bytecode must deploy on TronEvmFactory: {:?}",
            result.result
        );
        let out = result.result.output().unwrap();
        assert!(!out.is_empty(), "runtime code must be non-empty");
        // runtime-код тоже содержит TVM-guard в диспетчере функций
        assert!(out.iter().any(|b| *b == 0xd3), "runtime must contain 0xD3 guard");
    }

    #[test]
    fn vanilla_factory_fails_on_same_bytecode() {
        use alloy_evm::eth::EthEvmFactory;
        let mut evm = EthEvmFactory::default()
            .create_evm(CacheDB::<EmptyDB>::default(), EvmEnv::default());
        let result = evm.transact_raw(create_tx(creation_bytecode())).unwrap();
        assert!(
            !result.result.is_success(),
            "vanilla EVM must NOT execute TVM opcodes — иначе тест выше тавтологичен"
        );
    }

    #[test]
    fn iscontract_semantics() {
        // 0x30 ADDRESS, 0xD4 ISCONTRACT, 0x5f PUSH0, 0x52 MSTORE, PUSH1 32, PUSH0, 0xF3 RETURN
        // Внутри creation-фрейма ADDRESS — адрес создаваемого контракта; кода по нему ещё нет ⇒ 0.
        let code = hex::decode("30d45f5260205ff3").unwrap();
        let mut evm = TronEvmFactory::default()
            .create_evm(CacheDB::<EmptyDB>::default(), EvmEnv::default());
        let result = evm.transact_raw(create_tx(code)).unwrap();
        // RETURN из creation-фрейма трактуется как runtime-код: он равен 32 нулям (U256::ZERO от D4)
        let out = result.result.output().unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(U256::from_be_slice(out), U256::ZERO);
    }
}
```

Примечания для имплементера: точные имена API сверить по месту (`transact_raw` vs `transact` у трейта `alloy_evm::Evm`; форма `EvmEnv::default()` и дефолтный SpecId — если дефолт не покрывает PUSH0/0x5f, задать `SpecId::CANCUN` через `EvmEnv`/`CfgEnv`). Если `EthEvmFactory::default().create_evm` требует иных аргументов — привести оба теста к одинаковой форме вызова. Ассерты не ослаблять.

- [ ] **Step 3: Реализация tron.rs**

Структура файла (~160 строк) — по эскизу разведки; за образец структуры брать `crates/evm/core/src/evm/op.rs` (фабрика+NestedEvm) и `eth.rs` (FoundryEvmFactory):

```rust
//! Tron EVM factory: vanilla revm + TVM opcodes 0xD0-0xD4.
//!
//! tron-solc injects a TRC-10 guard (0xD3 CALLTOKENID / 0xD2 CALLTOKENVALUE)
//! into every non-payable entry, so executing tron-solc bytecode requires
//! these instructions. Stage 1.5 stubs: no TRC-10 exists in local tests,
//! so token value/id/balance are 0; ISCONTRACT is implemented faithfully;
//! CALLTOKEN halts loudly (NotActivated) instead of silently lying.
//! Costs mirror java-tron EnergyCost.java (BASE_TIER=2, BALANCE=20, CALL=40).

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
        inject_tron_opcodes(EthEvmFactory.create_evm(db, env), false)
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        inject_tron_opcodes(EthEvmFactory.create_evm_with_inspector(db, env, inspector), true)
    }
}

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

/// 0xD0 CALLTOKEN (8 args -> success): TRC-10 transfers are out of scope —
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

/// 0xD4 ISCONTRACT (address -> bool): faithful — true iff code is non-empty (TIP-44).
fn op_iscontract<W: InterpreterTypes, H: Host + ?Sized>(
    ctx: InstructionContext<'_, H, W>,
) -> Result<(), InstructionResult> {
    let Some(addr) = ctx.interpreter.stack.pop_address() else {
        return Err(InstructionResult::StackUnderflow);
    };
    let is_contract =
        ctx.host.load_account_code(addr).map(|c| !c.data.is_empty()).unwrap_or(false);
    if !ctx.interpreter.stack.push(U256::from(is_contract as u64)) {
        return Err(InstructionResult::StackOverflow);
    }
    Ok(())
}
```

Плюс (по разведке, дословные копии с заменой типа):
- `type TronCtx<'db> = EthEvmContext<&'db mut dyn DatabaseExt<TronEvmFactory>>;` и `pub type TronRevmEvm<'db, I> = RevmEvm<TronCtx<'db>, I, EthInstructions<EthInterpreter, TronCtx<'db>>, PrecompilesMap, EthFrame>;`
- `impl FoundryEvmFactory for TronEvmFactory` — копия `eth.rs:36-62` (тело `create_foundry_evm_with_inspector` идентично: `Self::default().create_evm_with_inspector(...)` уже попадает в наш impl с инъекцией);
- `impl NestedEvm for TronRevmEvm<'db, I>` — копия `eth.rs:64-104` с заменой `EthEvmFactory` → `TronEvmFactory` в bounds/`DatabaseExt<...>`.
- Импорты — собрать по eth.rs/op.rs + `revm::interpreter::{Instruction, InstructionContext, InstructionResult, InterpreterTypes}`, `revm::context_interface::Host` (точные пути реэкспортов сверить по месту; сигнатура функции-инструкции: `fn(InstructionContext<'_, H, W>) -> Result<(), InstructionResult>`, revm-interpreter-41.0.0/src/instructions.rs:37-44; стек: трейт `StackTr` — `push/pop/pop_address`; host: `load_account_code`, revm-context-interface-41.0.0/src/host.rs:180).

В `crates/evm/core/src/evm/mod.rs`:
- рядом с остальными модулями (~35-43): `pub mod tron;` + `pub use tron::*;` (по форме соседей);
- в `impl FoundryEvmNetwork for TronEvmNetwork` (~74-77): `type EvmFactory = TronEvmFactory;`.

- [ ] **Step 4: Тесты зелёные**

Run: `PATH="$TC:$PATH" cargo test -p foundry-evm-core tron`
Expected: PASS (3 passed) — tron-solc байткод деплоится на TronEvmFactory, падает на ванильной, ISCONTRACT возвращает 0/1 корректно.

- [ ] **Step 5: Потребители компилируются**

Run: `PATH="$TC:$PATH" cargo check -p forge -p forge-verify && PATH="$TC:$PATH" cargo clippy -p foundry-evm-core --all-targets 2>&1 | tail -5`
Expected: OK, без новых warnings. Если verify/bytecode.rs использовал `EthEvmNetwork` для Tron-арма (план C, Task 2) — заменить на `TronEvmNetwork` (теперь семантика различается!) и перепроверить.

- [ ] **Step 6: Commit**

```bash
git add crates/evm/core
git commit -m "feat(tron): TronEvmFactory with TVM opcodes 0xD0-0xD4 instruction stubs"
```

---

### Task 2: Закрыть E2E-гейт плана C (sandbox на настоящем tron-solc)

**Files:**
- Modify: `sandbox/tron-counter/foundry.toml` (убедиться: `solc` указывает на tron-solc)
- Modify: `sandbox/tron-counter/test/Counter.t.sol` (добавить тест на guard-опкоды)
- Modify: `docs/tron/STATUS.md` (статусы C/C2)

**Interfaces:**
- Consumes: Task 1; собранный `target/debug/forge`.
- Produces: пройденный гейт плана C — `forge build` + `forge test` на байткоде tron-solc; обновлённый STATUS.md.

- [ ] **Step 1: Дополнительный тест — guard исполняется, а не обходится**

В `sandbox/tron-counter/test/Counter.t.sol` добавить:

```solidity
    function testNonPayableGuardWithTvmOpcodes() public {
        // Любой вызов non-payable функции tron-solc-контракта проходит через
        // guard CALLVALUE -> CALLTOKENID (0xD3) -> CALLTOKENVALUE (0xD2).
        // Если бы опкоды не исполнялись, setNumber ревертил бы OpcodeNotFound.
        counter.setNumber(7);
        require(counter.number() == 7, "guard blocked a plain call");
    }
```

- [ ] **Step 2: Пересобрать forge и прогнать гейт**

```bash
TC=$(dirname "$(rustup which --toolchain stable cargo)")
PATH="$TC:$PATH" cargo build -p forge --bin forge
cd sandbox/tron-counter
rm -rf out cache
../../target/debug/forge build   # компилятор — tron-solc из foundry.toml
../../target/debug/forge test -vv
```

Expected: build OK (в выводе/artifacts — версия компилятора 0.8.27, путь tron-solc); `forge test` — **4/4 passed** (testIncrement, testTronChainId, testTransientStorageCancun, testNonPayableGuardWithTvmOpcodes). Это и есть гейт плана C: тестируется именно тот байткод, который деплоится на Tron.

- [ ] **Step 3: Санити-контраст**

Временно вернуть `TronEvmNetwork::EvmFactory = EthEvmFactory` НЕ нужно (уже доказано юнит-тестом `vanilla_factory_fails_on_same_bytecode`). Вместо этого: убедиться, что артефакт собран именно tron-solc — `python3 -c "import json;a=json.load(open('out/Counter.sol/Counter.json'));print(a['metadata']['compiler'])"` и что deployedBytecode содержит `d3`.

- [ ] **Step 4: Обновить STATUS.md**

В `docs/tron/STATUS.md`: план C — ✅ полностью (гейт закрыт через C2), план C2 — ✅; дальше план D. Отразить дату.

- [ ] **Step 5: Commits + push**

```bash
git add sandbox/tron-counter docs/tron/STATUS.md
git commit -m "feat(tron): close plan C gate — sandbox E2E on real tron-solc bytecode"
git push origin tron-dev
```

---

## Критерий завершения плана C2

`cargo test -p foundry-evm-core tron` — 3/3; sandbox `forge test` — 4/4 на байткоде tron-solc; `cargo check --workspace` чистый; STATUS.md обновлён и запушен. После этого — план D (cast/forge script: деплой на Nile через tron-provider).
