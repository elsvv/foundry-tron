# Tron network in forge Implementation Plan (План C этапа 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `forge test` и `forge build` работают с `network = "tron"`: вариант `NetworkVariant::Tron`, маркер-тип `TronEvmNetwork` (ванильный revm/Cancun, chain id 728126428), компиляция реальным tron-solc (нативный `solc-macos` 0.8.27), E2E-валидация на sample-проекте.

**Architecture:** «Наивный» Tron = конфигурация, а не новая VM: `TronEvmNetwork` переиспользует `alloy_network::Ethereum` и `EthEvmFactory`; spec задаётся `evm_version = "cancun"` → `SpecId::CANCUN`, chain id — конфигом. Никаких правок hardforks/primitives/tempo.rs. Добавление варианта в `NetworkVariant` ломает три исчерпывающих match вне forge — чиним все. tron-solc подключается как `SolcReq::Local` без правок кода компиляции.

**Tech Stack:** Rust (крейты foundry-evm-networks, foundry-evm-core, forge, forge-verify, cast), tron-solc `solc-macos` 0.8.27 (Intel-бинарь, Rosetta 2), Solidity sample без forge-std.

**Спека:** `docs/superpowers/specs/2026-07-10-foundry-tron-fork-design.md` (секции 4.4 этап 1, 4.3, 4.7).
**Разведка (факты с номерами строк):** отчёт scout-plan-c в истории сессии; ключевые точки продублированы в задачах.

## Global Constraints

- Пути — относительно `/Users/vaceslaveliseev/@dev/foundry-tron/foundry/`; ветка `tron-dev`.
- **Тулчейн:** все cargo-команды с PATH-фиксом: `TC=$(dirname "$(rustup which --toolchain stable cargo)"); PATH="$TC:$PATH" cargo <...>`. Форматирование — nightly rustfmt как в планах A/B.
- Tron mainnet chain id: `728126428` (0x2b6653dc).
- Правки существующих файлов foundry — минимальные и точечные (это форк, каждая лишняя строка — конфликт при rebase). Новых зависимостей нет.
- Компиляция крейтов тяжёлая: для быстрой итерации использовать `cargo check -p <crate>`; полный `cargo check --workspace` — один раз в конце соответствующей задачи. `cargo build -p forge --bin forge` собирать один раз (Task 4) и переиспользовать бинарь `target/debug/forge`.
- Sample-проект для E2E — в `/Users/vaceslaveliseev/@dev/foundry-tron/sandbox/tron-counter/` (внешний репо, коммитится туда).
- Коммиты — conventional commits `feat(tron):`/`fix(tron):`; код foundry — в foundry-репо, sandbox — во внешнем.

---

### Task 1: Clippy-чистка крейтов планов A/B (долг перед CI)

**Files:**
- Modify: `crates/tron/primitives/src/units.rs`, `crates/tron/primitives/src/proto.rs`, `crates/tron/primitives/src/address.rs` (по фактическому выводу clippy)

**Interfaces:**
- Consumes: — Produces: `cargo clippy -p foundry-tron-primitives -p foundry-tron-provider --all-targets` без warnings (CI идёт с `RUSTFLAGS=-Dwarnings`; Fable-ревью планов A/B зафиксировали ~6 warnings: `missing_const_for_fn` в units/proto, `derive_partial_eq_without_eq` в proto, `unnested_or_patterns` в тесте address). Публичные сигнатуры НЕ меняются, кроме допустимого добавления `const` к fn и `Eq` к derive.

- [ ] **Step 1: Снять фактический список**

```bash
TC=$(dirname "$(rustup which --toolchain stable cargo)")
PATH="$TC:$PATH" cargo clippy -p foundry-tron-primitives -p foundry-tron-provider --all-targets 2>&1 | grep -E "^warning|^error" | sort | uniq -c
```

- [ ] **Step 2: Исправить каждый warning**

По типам: `missing_const_for_fn` → добавить `const` (например `pub const fn trx_to_sun`, `pub const fn type_url`); `derive_partial_eq_without_eq` → добавить `Eq` в derive, где все поля Eq (prost-структуры: проверить, что `prost::Message`-derive не конфликтует — если конфликтует, точечный `#[allow]` с комментарием-причиной); `unnested_or_patterns` → объединить паттерны. Тестов не ослаблять; поведение не менять.

- [ ] **Step 3: Проверка**

Run: `PATH="$TC:$PATH" cargo clippy -p foundry-tron-primitives -p foundry-tron-provider --all-targets 2>&1 | tail -5`
Expected: ноль warnings. Затем `cargo test -p foundry-tron-primitives -p foundry-tron-provider` — все зелёные (33 суммарно).

- [ ] **Step 4: Commit**

```bash
git add crates/tron
git commit -m "fix(tron): clear clippy warnings in tron crates for -Dwarnings CI"
```

---

### Task 2: NetworkVariant::Tron + все исчерпывающие match

**Files:**
- Modify: `crates/evm/networks/src/lib.rs` (enum ~105-136, `From<NetworkVariant> for NetworkConfigs` ~376-389, хелперы ~213)
- Modify: `crates/verify/src/bytecode.rs` (~174-185)
- Modify: `crates/cast/src/cmd/da_estimate.rs` (~39-45)
- Modify: `crates/cast/src/args.rs` (~941-956, DecodeTransaction)

**Interfaces:**
- Consumes: —
- Produces: `NetworkVariant::Tron`; `"tron".parse::<NetworkVariant>()` работает; `NetworkVariant::Tron.name() == "tron"`; `NetworkConfigs::is_tron() -> bool` и `NetworkConfigs::with_tron() -> Self`; workspace компилируется. (Точные номера строк — из разведки; если код сдвинулся, ориентироваться на структуры: искать `enum NetworkVariant`, `impl FromStr`, `fn name`, `impl From<NetworkVariant> for NetworkConfigs`, `fn is_tempo`.)

- [ ] **Step 1: Падающий тест**

В `crates/evm/networks/src/lib.rs` в конец (или в существующий `#[cfg(test)] mod tests`, если есть):

```rust
#[cfg(test)]
mod tron_tests {
    use super::*;

    #[test]
    fn tron_variant_parses_and_names() {
        let v: NetworkVariant = "tron".parse().unwrap();
        assert_eq!(v, NetworkVariant::Tron);
        assert_eq!(v.name(), "tron");
    }

    #[test]
    fn tron_configs_flag() {
        let c = NetworkConfigs::with_tron();
        assert!(c.is_tron());
        assert!(!c.is_tempo());
        let via_from: NetworkConfigs = NetworkVariant::Tron.into();
        assert!(via_from.is_tron());
    }
}
```

- [ ] **Step 2: Убедиться, что падает**

Run: `PATH="$TC:$PATH" cargo test -p foundry-evm-networks tron_`
Expected: FAIL — ошибки компиляции (нет варианта `Tron`, нет `is_tron`/`with_tron`).

- [ ] **Step 3: Реализация в evm/networks**

В `crates/evm/networks/src/lib.rs`, по образцу соседних вариантов:
- `enum NetworkVariant { ..., Tron }` (сохранить существующие derive/serde-атрибуты соседних вариантов; если у enum есть `#[serde(rename_all = ...)]` — ничего дополнительно не нужно);
- `FromStr`: `"tron" => Ok(Self::Tron),`;
- `name()`: `Self::Tron => "tron",`;
- `impl From<NetworkVariant> for NetworkConfigs`: арм `NetworkVariant::Tron => Self { network: Some(network), ..Default::default() },` (точно по форме соседних армов — сверить с фактическим кодом);
- рядом с `is_tempo` (по его образцу):

```rust
/// Returns true when the Tron network is selected.
pub const fn is_tron(&self) -> bool {
    matches!(self.resolved_network(), Some(NetworkVariant::Tron))
}

/// Creates configs with the Tron network enabled.
pub fn with_tron() -> Self {
    Self { network: Some(NetworkVariant::Tron), ..Default::default() }
}
```

(Если `resolved_network()`/поле называются иначе — следовать фактической реализации `is_tempo`.)

- [ ] **Step 4: Тесты networks зелёные**

Run: `PATH="$TC:$PATH" cargo test -p foundry-evm-networks`
Expected: PASS, включая оба новых.

- [ ] **Step 5: Починить сломанные исчерпывающие match**

Проверить компиляцию потребителей: `PATH="$TC:$PATH" cargo check -p forge-verify -p cast 2>&1 | grep -E "^error" -A 3 | head -40`. Ожидаемые три поломки и их фиксы (наивный Tron = ванильный EVM → маппим на Ethereum-пути):

`crates/verify/src/bytecode.rs` (match по network, ~174-185):

```rust
NetworkVariant::Tron => self.run_with_network_and_config::<TronEvmNetwork>(config).await,
```

(импортировать `TronEvmNetwork` оттуда же, откуда `TempoEvmNetwork`; если задача 3 ещё не влита — временно использовать `EthEvmNetwork` НЕЛЬЗЯ, порядок задач в плане гарантирует, что Task 3 идёт после; в этой задаче поставить арм с `EthEvmNetwork` и TODO-комментарий ЗАПРЕЩЁН — вместо этого выполнить Task 2 Step 5 ПОСЛЕ Task 3 невозможно, поэтому: здесь используем `EthEvmNetwork` осознанно и НАВСЕГДА — для verify наивного этапа это корректная семантика, поведение идентично; комментарий в коде: `// naive Tron executes as vanilla EVM`).

`crates/cast/src/cmd/da_estimate.rs` (~39-45):

```rust
NetworkVariant::Tron => da_estimate::<Ethereum>(&config, block).await,
```

`crates/cast/src/args.rs` (~941-956, DecodeTransaction, match БЕЗ `_`):

```rust
Some(NetworkVariant::Tron) => SimpleCast::decode_raw_transaction::<Ethereum>(&tx)?,
```

(во всех трёх — форма арма точно по соседнему Ethereum-арму фактического кода).

- [ ] **Step 6: Компиляция потребителей**

Run: `PATH="$TC:$PATH" cargo check -p forge-verify -p cast -p foundry-evm-networks`
Expected: OK без ошибок и warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/evm/networks crates/verify crates/cast
git commit -m "feat(tron): add NetworkVariant::Tron with config helpers"
```

---

### Task 3: TronEvmNetwork + диспетчеризация в forge test

**Files:**
- Modify: `crates/evm/core/src/evm/mod.rs` (рядом с `EthEvmNetwork`, ~63-68)
- Modify: `crates/forge/src/cmd/test/mod.rs` (`NetworkDispatchKind` ~262-268, `network_dispatch_kind` ~270-281, `dispatch_network` ~2494-2514, `dispatch_fuzz_minimize_network` ~2526-2540)

**Interfaces:**
- Consumes: `NetworkConfigs::is_tron()` (Task 2)
- Produces: `foundry_evm_core::evm::TronEvmNetwork` — маркер-тип: `type Network = Ethereum; type EvmFactory = EthEvmFactory;` (новых impl `FoundryEvmFactory`/`NestedEvm` НЕ требуется — они уже есть для `EthEvmFactory`); `forge test` при `network = "tron"` мономорфизируется в `build_and_run_tests::<TronEvmNetwork>`.

- [ ] **Step 1: TronEvmNetwork**

В `crates/evm/core/src/evm/mod.rs` по образцу `EthEvmNetwork` (сверить фактические имя трейта и associated types на месте):

```rust
/// Tron network marker. Naive stage: executes as vanilla EVM (Cancun);
/// chain id and compiler come from config. TVM specifics land in tron-revm later.
#[derive(Clone, Copy, Debug, Default)]
pub struct TronEvmNetwork;

impl FoundryEvmNetwork for TronEvmNetwork {
    type Network = Ethereum;
    type EvmFactory = EthEvmFactory;
}
```

Run: `PATH="$TC:$PATH" cargo check -p foundry-evm-core` → OK.

- [ ] **Step 2: Диспетчер в forge**

В `crates/forge/src/cmd/test/mod.rs` (формы армов — по соседним Tempo/Eth):
- `enum NetworkDispatchKind`: добавить `Tron,`;
- `network_dispatch_kind`: первой проверкой `if evm_opts.networks.is_tron() { return NetworkDispatchKind::Tron; }`;
- `dispatch_network`: арм `NetworkDispatchKind::Tron => self.build_and_run_tests::<TronEvmNetwork>(config, evm_opts, output, filter, execution).await,`;
- `dispatch_fuzz_minimize_network`: аналогичный арм через `TronEvmNetwork`;
- импорт `TronEvmNetwork` рядом с `TempoEvmNetwork`.

Если в `crates/verify/src/bytecode.rs` в Task 2 использован `EthEvmNetwork` — заменить на `TronEvmNetwork` сейчас (семантика та же, но шов именованный; проверить `cargo check -p forge-verify`).

- [ ] **Step 3: Компиляция forge**

Run: `PATH="$TC:$PATH" cargo check -p forge`
Expected: OK. Затем полный `PATH="$TC:$PATH" cargo check --workspace 2>&1 | tail -5` — OK (ловим оставшиеся исчерпывающие match, если разведка что-то пропустила; при новых поломках — чинить тем же паттерном «наивный Tron = Ethereum-путь» и зафиксировать в summary).

- [ ] **Step 4: Commit**

```bash
git add crates/evm/core crates/forge crates/verify
git commit -m "feat(tron): TronEvmNetwork marker and forge test dispatch"
```

---

### Task 4: tron-solc + sample-проект + E2E `forge build/test --network tron`

**Files:**
- Create: `~/.foundry-tron/solc/tron-solc-0.8.27` (бинарь, вне git)
- Create: `/Users/vaceslaveliseev/@dev/foundry-tron/sandbox/tron-counter/foundry.toml`
- Create: `/Users/vaceslaveliseev/@dev/foundry-tron/sandbox/tron-counter/src/Counter.sol`
- Create: `/Users/vaceslaveliseev/@dev/foundry-tron/sandbox/tron-counter/test/Counter.t.sol`

**Interfaces:**
- Consumes: всё из Tasks 2-3; `target/debug/forge`
- Produces: доказательство E2E: sample-проект компилируется настоящим tron-solc и `forge test` проходит с `network = "tron"`, включая ассерт `block.chainid == 728126428` внутри Solidity-теста (доказывает прокладку chain id до EVM). Это gate плана C.

- [ ] **Step 1: Скачать tron-solc (нативный macOS-бинарь)**

```bash
mkdir -p ~/.foundry-tron/solc
curl -sL -o ~/.foundry-tron/solc/tron-solc-0.8.27 \
  "https://github.com/tronprotocol/solidity/releases/download/0.8.27_Democritus_v4.8.1/solc-macos"
chmod +x ~/.foundry-tron/solc/tron-solc-0.8.27
~/.foundry-tron/solc/tron-solc-0.8.27 --version
```

Expected: печатает версию 0.8.27 (это Intel-бинарь — на Apple Silicon исполняется через Rosetta 2). Если тег/имя ассета отличаются — найти точные: `curl -s https://api.github.com/repos/tronprotocol/solidity/releases/latest | python3 -c "import json,sys;d=json.load(sys.stdin);print(d['tag_name']);[print(a['name'],a['browser_download_url']) for a in d['assets']]"`. Если бинарь не запускается (нет Rosetta) — зафиксировать в summary, поставить Rosetta нельзя без пользователя → fallback: официальный solc (`svm install 0.8.27` или существующий в системе), с пометкой в foundry.toml-комментарии, и явно отразить в результате задачи.

- [ ] **Step 2: Sample-проект (без forge-std — ноль внешних зависимостей)**

`sandbox/tron-counter/foundry.toml`:

```toml
[profile.default]
src = "src"
out = "out"
test = "test"
libs = []
network = "tron"
chain_id = 728126428
evm_version = "cancun"
solc = "~/.foundry-tron/solc/tron-solc-0.8.27"
```

(Если foundry не разворачивает `~` в SolcReq::Local — вписать абсолютный путь `/Users/vaceslaveliseev/.foundry-tron/solc/tron-solc-0.8.27`.)

`src/Counter.sol`:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

contract Counter {
    uint256 public number;

    function setNumber(uint256 newNumber) public {
        number = newNumber;
    }

    function increment() public {
        number++;
    }
}
```

`test/Counter.t.sol` (без forge-std: forge находит контракты `*Test` и функции `test*`; падение = revert):

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Counter} from "../src/Counter.sol";

contract CounterTest {
    Counter internal counter;

    function setUp() public {
        counter = new Counter();
        counter.setNumber(41);
    }

    function testIncrement() public {
        counter.increment();
        require(counter.number() == 42, "increment failed");
    }

    function testTronChainId() public view {
        require(block.chainid == 728126428, "chain id is not Tron mainnet");
    }

    function testTransientStorageCancun() public {
        // TSTORE/TLOAD доступны только с Cancun — доказывает evm_version
        assembly {
            tstore(0, 42)
            if iszero(eq(tload(0), 42)) { revert(0, 0) }
        }
    }
}
```

- [ ] **Step 3: Собрать forge и прогнать E2E**

```bash
cd /Users/vaceslaveliseev/@dev/foundry-tron/foundry
TC=$(dirname "$(rustup which --toolchain stable cargo)")
PATH="$TC:$PATH" cargo build -p forge --bin forge      # долго, один раз
cd /Users/vaceslaveliseev/@dev/foundry-tron/sandbox/tron-counter
/Users/vaceslaveliseev/@dev/foundry-tron/foundry/target/debug/forge build
/Users/vaceslaveliseev/@dev/foundry-tron/foundry/target/debug/forge test -vv
```

Expected: `forge build` компилирует оба контракта tron-solc'ом без ошибок; `forge test` — 3 passed (testIncrement, testTronChainId, testTransientStorageCancun). Диагностика типовых падений: testTronChainId падает → chain id не дошёл до EVM: проверить, что `chain_id` из конфига попадает в `evm_opts.env.chain_id` (см. `crates/evm/core/src/opts.rs`, `local_evm_env`), при необходимости прогнать `forge test --chain-id 728126428` и зафиксировать разницу; tron-solc не принимает standard-json → снять точную ошибку и сравнить с обычным solc 0.8.27.

- [ ] **Step 4: Санити — сеть реально Tron-ветка**

```bash
cd sandbox/tron-counter
/Users/vaceslaveliseev/@dev/foundry-tron/foundry/target/debug/forge test -vv 2>&1 | head -5
grep -n "network" foundry.toml
```

Дополнительно убедиться кодом: временно поменять `network = "tempo"` → `forge test` должен повести себя иначе (другая ветка диспетчера; вероятна ошибка Tempo-инициализации) — вернуть обратно `"tron"`. Зафиксировать наблюдение в summary (доказывает, что `network` не игнорируется).

- [ ] **Step 5: Commits (два репо)**

```bash
cd /Users/vaceslaveliseev/@dev/foundry-tron/foundry
git add -A && git diff --cached --stat   # убедиться: пусто или только осознанные правки (например opts.rs)
# коммитить в foundry только если были правки кода в Step 3-диагностике:
# git commit -m "fix(tron): <что именно>"
cd /Users/vaceslaveliseev/@dev/foundry-tron
git add sandbox/tron-counter
git commit -m "feat(sandbox): tron-counter sample proving forge build/test with network=tron"
```

---

## Критерий завершения плана C

`cargo check --workspace` чистый; `cargo test -p foundry-evm-networks` зелёный; sample-проект: `forge build` компилирует tron-solc'ом, `forge test` — 3/3 passed с `network = "tron"`, chain id 728126428 и Cancun подтверждены изнутри EVM. После этого — план D (cast/forge script: деплой на Nile через tron-provider).
