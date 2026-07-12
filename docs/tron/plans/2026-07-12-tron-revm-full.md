# Full tron-revm Plan (План E — Этап 2, ядро: energy-модель, precompiles, CREATE2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** превратить наивный `TronEvmFactory` (этап 1.5: только опкоды 0xD0–0xD4) в верный tron-revm: точная **energy-модель** (Tron = pre-EIP-150), **полный набор precompiles** java-tron, **CREATE2 по формуле Tron** — с golden-подтверждением против ноды. Это ядро Этапа 2 спеки (§6); резолвер tron-solc и fork-режим — отдельными планами.

**Спека:** `docs/tron/specs/2026-07-10-foundry-tron-fork-design.md` §4.4, §6. Разведка: scout-stage2 (7 отчётов, 2026-07-12). Ветка: **`tron-stage2`**.

## Верифицированные доменные факты (перепроверены по первоисточникам; НЕ пересматривать без них)

Источник java-tron @develop доступен через **jsdelivr** (raw.githubusercontent с этой машины блокируется): `https://cdn.jsdelivr.net/gh/tronprotocol/java-tron@develop/<path>`. Live-пробы: `api.trongrid.io` (mainnet), `api.nileex.io` (Nile; `/jsonrpc` там НЕ смонтирован — только `/wallet/*`).

1. **Energy-модель Tron = pre-EIP-150 Ethereum (FRONTIER) + TVM-дельты.** `EnergyCost.java`: SLOAD=50, BALANCE=20, EXTCODEHASH=400, CALL base=40 (+9000 value, +25000 dead acct, stipend 2300), SSTORE flat SET=20000/CLEAR=5000/RESET=5000 (без EIP-2200/2929, БЕЗ cold/warm), SHA3=30+6/word, LOG=375+375/topic+8/byte, EXP=10+**10**/byte (ETH: 50/byte), CREATE=32000, CREATE_DATA=200/byte, TLOAD/TSTORE=100, память `3w+w²/512` (=ETH), MEM_LIMIT=3MB, SUICIDE_V2=5000(+25000). **Спека §4.4 «energy = revm-gas 1:1» — НЕВЕРНА** для CANCUN-пути (передозировка 20–50× на storage/account ops).
2. **revm 41 tri-table — подмена данных, не логики:** `InstructionTable` (fn, без газа) + `GasTable [u16;256]` (статический газ, `EthInstructions::gas_table_mut()`, `insert_instruction(op,instr,gas)` пишет оба) + `GasParams` (динамический, `CfgEnv::set_gas_params`, `GasParams::new_spec(SpecId::FRONTIER)`, `override_gas([(GasId,u64)])`). FRONTIER-базис revm совпадает с EnergyCost.java по всем перечисленным позициям, кроме: EXP byte (revm CANCUN 50 → 10), refunds (FRONTIER несёт sstore_clearing 15000/selfdestruct 24000 — у Tron refund'ов НЕТ: подтвердить по Program.java/ProgramResult прежде чем занулять), intrinsic 21000+calldata (у Tron это bandwidth, НЕ energy → занулить tx_base_stipend/token costs), SELFDESTRUCT 5000, TLOAD/TSTORE 100.
3. **Live chain parameters (mainnet и Nile ИДЕНТИЧНЫ, проба 2026-07-12):** `getEnergyFee=100` SUN/energy (**спека говорила 420 — неверно**), `getTransactionFee=1000` SUN/байт bandwidth, максимум fee_limit 15000 TRX. Активны: Shanghai/Cancun/Blob/SelfdestructRestriction/DynamicEnergy; НЕ активны: Osaka (CLZ), CompatibleEvm, FreezeV1.
4. **Блочно-tx опкоды Tron:** DIFFICULTY/PREVRANDAO 0x44 → **всегда 0**; GASLIMIT 0x45 → **всегда 0**; BASEFEE 0x48 → `getEnergyFee()`=**100**; GASPRICE 0x3a → **0** (CompatibleEvm выключен на mainnet); BLOBHASH 0x49 → pop 1, push **0**; BLOBBASEFEE 0x4a → push **0** (оба «live-but-stubbed» в java-tron `OperationActions.java:692,698`). CHAINID — config-driven, уже верно.
5. **Лимиты:** НЕТ EIP-170 (24KB cap) и НЕТ EIP-3860 (initcode metering) — `Program.java:927-943`, `EnergyCost:415`; call depth **64** (`MAX_DEPTH=64`, `Program.java:109`) против 1024 в revm (`CALL_STACK_LIMIT`, revm-handler frame.rs); 63/64-правило НЕ применяется (CompatibleEvm выключен) — версия-0 контракты форвардят всю энергию.
6. **Precompiles java-tron (`PrecompiledContracts.java`, полная таблица в отчёте scout-stage2/java-tron-precompiles.md):**
   - `0x01` ECRecover 3000 — **проверить формат выхода live** (может отдавать 21-байтовый 0x41-адрес в word);
   - `0x02` Sha256 60+12/word (= ETH);
   - `0x03` — **НЕ ripemd160!** Это `sha256(sha256(data)[0..20])`, 600+120/word (`PC.java:564-577`);
   - `0x04` Identity 15+3/word (= ETH);
   - `0x05` ModExp — EIP-198, GQUAD_DIVISOR=**20**, **без 200-floor** (ETH Berlin: divisor 3, floor 200);
   - `0x06/0x07/0x08` bn128 — Istanbul-цены 150/6000/34000·k+45000 (= ETH Istanbul);
   - `0x09` **BatchValidateSign** (TIP-43), energy `((len/32-5)/6)*1500` — ЗАМЕЩАЕТ blake2f;
   - `0x0a` **ValidateMultiSign** (TIP-60), energy `((len/32-5)/5)*1500` — ЗАМЕЩАЕТ KZG point-eval; требует permission-стейта аккаунта;
   - `0x100` P256Verify — Osaka-gated, на mainnet НЕ активен → не ставить;
   - `0x020003` EthRipemd160 (настоящий ripemd160, 600+120/word), `0x020009` Blake2F (EIP-152) — активация `allowTvmCompatibleEvm`, на mainnet флаг НЕ активен → вопрос активации решить по факту: ставим (безвредно, адреса вне досягаемости обычного кода) с пометкой;
   - shielded `0x1000001-4` (150000/200000/150000/500) и vote/freeze `0x1000005-0x1000015` (500/50/20) — **стабы**.
7. **CREATE2 Tron:** `addr20 = keccak256( sender21_0x41 ‖ salt32 ‖ keccak256(initcode) )[12..]` — **НЕТ 0xff**, sender 21-байтовый (`WalletUtil.generateContractAddress2`, `Program.java:1618-1634`, Istanbul: sender = адрес исполняющегося контракта). **Внутренний CREATE**: `keccak256(rootTxId32 ‖ nonce_be8)[12..]`, где nonce — глобальный op-счётчик root-транзакции (инкремент на CALL/CREATE/SUICIDE/freeze/vote) — **локально невоспроизводим**; решение: CREATE остаётся EVM-схемой локально (документированная дельта), CREATE2 — честный.
8. **revm 41 seam для CREATE2:** `CreateScheme::Custom { address }` существует (`revm-context-interface/src/cfg.rs:116-130`), уважается и `make_create_frame` (frame.rs:309), и cheatcode-инспектором foundry (`inspector/utils.rs:60-70` через `created_address`). Форк revm НЕ нужен. Рекомендованный механизм — кастомная инструкция 0xF5 (копия стоковой `create<true>` из `revm-interpreter/src/instructions/contract.rs:26-129` с постройкой `CreateInputs` со схемой Custom) — гарантирует консистентность до любых инспекторов.
9. **Precompile-инжекция:** `PrecompilesMap::extend_precompiles/apply_precompile` + `DynPrecompile::new(id, fn)` (чистые) / `new_stateful` (со state через `PrecompileInput::internals()` → `EvmInternals`). Инжектировать в `inject_tron_opcodes` (tron.rs:74; переименовать в `inject_tron_extensions`) — покрывает оба пути (`create_evm` и `create_evm_with_inspector`); поле `inner.precompiles` публично. НЕ добавлять в `NetworkConfigs::inject_precompiles` (только foundry-путь — split-brain). Labels — `NetworkConfigs::precompiles_label`/`precompiles` (is_tron ветка, зеркало TEMPO_PRECOMPILES).
10. **Нефатальность precompile-ошибок:** неверный вход → `PrecompileOutput::halt(...)`/`revert`, НЕ `Err(PrecompileError)` (fatal, абортит tx — расхождение с java-tron). Прецедент: `crates/evm/networks/src/celo/transfer.rs`.

## Global Constraints

- Репо `/Users/andrey/vibe_projects/foundry-tron`, ветка **`tron-stage2`**. Коммитить локально, НЕ пушить (оркестратор пушит и открывает PR `tron-stage2` → `tron-dev-continue`).
- Тулчейн: `export PATH="$HOME/.cargo/bin:$PATH"`; `cargo +nightly fmt`; долгие сборки timeout 600000 c перезапуском. `cast` = пакет `cast@1.7.2`.
- Live: `.env.tron-dev` (git-ignored), `TRON_LIVE=1`, Nile-нода **`https://api.nileex.io`** (nile.trongrid.io с этой машины недоступен), баланс ~1900 TRX. Экономно: маленькие контракты, fee_limit ≤ 400 TRX.
- Тесты: реальные векторы (java-tron исходники/тесты, live-нода); запрещены тавтология и ослабление ассертов, `#[ignore]` запрещён; live-гейты с явным eprintln-skip.
- Минимальные диффы вне tron-кода; Ethereum/Optimism/Tempo-пути не трогать (их тесты должны остаться зелёными). Никаких новых внешних зависимостей (ripemd/blake2f/bn128/modexp реализации БРАТЬ из revm-precompile, он уже в дереве).
- Конвенции CLAUDE.md; conventional commits c `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

## Architecture

- `crates/evm/core/src/evm/tron.rs` → директория `crates/evm/core/src/evm/tron/{mod.rs, energy.rs, precompiles.rs, create.rs}` (mod.rs сохраняет текущий публичный API: TronEvmFactory, TronRevmEvm и т.д.). `inject_tron_opcodes` → `inject_tron_extensions`: опкоды + gas-таблицы + precompiles + CREATE2-инструкция в одном месте (оба пути create_evm).
- Energy: `*instructions.gas_table_mut() = FRONTIER-базис` → re-apply TVM inserts → `cfg.set_gas_params(GasParams::new_spec(FRONTIER))` → `override_gas` дельт. Spec остаётся CANCUN (опкоды/журналирование), газ — FRONTIER+дельты.
- Precompiles: чистые — `DynPrecompile::new`; стейтовые/стабы — `new_stateful`; loud-fail стабы возвращают revert с сообщением (не fatal).
- CREATE2 — кастомная инструкция 0xF5 → `CreateScheme::Custom{tron_addr}`; CREATE — стоковый (EVM-схема, документированная локальная дельта); cheatcode `computeCreate2Address*` — специализация на Tron-путь.

---

### Task 1: Energy-модель (FRONTIER-таблицы + TVM-дельты + лимиты + блочные стабы)

**Files:**
- Refactor: `crates/evm/core/src/evm/tron.rs` → `tron/mod.rs` + Create: `tron/energy.rs`
- Tests: внутри `tron/energy.rs` + live golden в `crates/tron/provider` (Task 4 доведёт)

**Steps:**

- [ ] **Step 1: Подтвердить refund-семантику java-tron** (обязательный доменный чек до кода): по jsdelivr прочитать `Program.java`/`ProgramResult.java`/энерго-процессор — есть ли futureRefund/refund у SSTORE clear и SELFDESTRUCT. Ожидание по разведке: refund'ов нет → занулить оба GasId. Результат зафиксировать в комментарии кода со ссылкой.
- [ ] **Step 2: Падающие тесты энергии.** Ручные векторы по EnergyCost.java (без сети): байткод-сниппеты через `TronEvmFactory::create_evm` + `transact_raw`, ассерт `result.gas_used()` == вручную посчитанная энергия (учесть, что intrinsic занулён):
  - SLOAD холодного слота = 50 (не 2100); BALANCE = 20 (не 2600); SSTORE 0→x = 20000, x→0 = 5000 и **без refund**; EXP с 1-байтовой экспонентой = 10+10; TLOAD/TSTORE = 100; SHA3 32 байта = 30+6; контраст-тест: те же сниппеты на `EthEvmFactory` дают ДРУГИЕ числа (нетавтологичность).
  - Деплой-цена: creation Counter fixture — energy = исполнение + 200·len(runtime) (CREATE_DATA), без EIP-3860 слагаемых.
- [ ] **Step 3: Реализация.** `tron/energy.rs`: fn `apply_tron_energy(inner: &mut RevmEvm...)`: gas_table FRONTIER (взять `revm::interpreter::gas_table()` FRONTIER-вариант — точное API сверить по месту: `gas_table_spec(SpecId::FRONTIER)`), re-apply 5 TVM inserts, TLOAD/TSTORE=100 в таблице (FRONTIER-базис может нести для них 0/спековые значения — выставить руками), SELFDESTRUCT=5000; `GasParams::new_spec(FRONTIER)` + `override_gas`: EXP byte→10, refunds→0 (по Step 1), intrinsic (tx_base_stipend + calldata token costs)→0, new_account_for_selfdestruct→25000. Лимиты: `cfg.limit_contract_code_size = None`-эквивалент (точное поле CfgEnv сверить), отключить EIP-3860 гейт (если он в инструкции create — учтётся в Task 3 CREATE2-инструкции; для стокового CREATE — через cfg, сверить наличие `max_initcode_size`). Investigate call depth 64: если в revm 41 depth-лимит переопределяем дёшево (cfg/handler-параметр) — сделать; если требует форка handler — зафиксировать known limitation в коде+STATUS. Блочные стабы: инструкции-override DIFFICULTY→push 0, GASLIMIT→push 0, BASEFEE→push 100, GASPRICE→push 0, BLOBHASH→pop+push 0, BLOBBASEFEE→push 0 (в `inject_tron_extensions`; gas — по java-tron tier'ам: BASE=2 для всех этих).
- [ ] **Step 4:** `cargo test -p foundry-evm-core tron` — все зелёные (старые 3 + новые энергетические); `cargo check -p forge -p forge-verify`; clippy; sandbox `forge test` в tron-counter по-прежнему 4/4 (пересобрать forge).
- [ ] **Step 5:** fmt + commit `feat(tron): faithful TVM energy model via FRONTIER gas tables`.

---

### Task 2: Precompiles java-tron

**Files:**
- Create: `crates/evm/core/src/evm/tron/precompiles.rs`
- Modify: `tron/mod.rs` (инжекция в `inject_tron_extensions`)
- Modify: `crates/evm/networks/src/lib.rs` (`TRON_PRECOMPILES` таблица; is_tron ветки в `precompiles_label`/`precompiles`)

**Steps:**

- [ ] **Step 1: Доменные векторы.** Из java-tron тестов (`framework/src/test/.../PrecompiledContractsTest.java` и соседних, через jsdelivr) выдернуть реальные входы/выходы для 0x03 (двойной sha256), 0x05 ModExp, 0x09 BatchValidateSign, 0x0a ValidateMultiSign. Дополнительно live-векторы: `trigger_constant` staticcall-обёрткой на адреса 0x01–0x0a на Nile (контракт-прокси не нужен — triggerconstantcontract умеет вызывать precompile-адрес напрямую с data; если нет — задеплоить мини-прокси). КРИТИЧНО: живой вектор для 0x01 ECRecover — проверить, 20 или 21 байт (0x41-префикс) в возвращаемом word, и для 0x03 — подтвердить двойной sha256. Зафиксировать в testdata JSON.
- [ ] **Step 2: Падающие тесты** на векторах Step 1: по каждому precompile — вход → выход + energy cost; контраст: на `EthEvmFactory` 0x03/0x05/0x09/0x0a дают другой результат/ошибку (нетавтологичность).
- [ ] **Step 3: Реализация** `precompiles.rs`: константы адресов; таблица `tron_precompiles() -> Vec<(Address, DynPrecompile)>`:
  - переиспользовать revm-precompile: sha256, ripemd160 (для 0x020003), identity, bn128, blake2f (0x020009), secp256k1 recover (0x01 — с output-форматом по Step 1), modexp core (0x05 с divisor 20 без floor — если revm API не параметризуется, реализовать формулу цены самим, math — из revm);
  - 0x03: своя (sha256(sha256(data)[0..20]) правопаддинг в word — точный формат из PC.java);
  - 0x09 BatchValidateSign: формат входа/выхода из PC.java:1080-1135 (hash, N подписей, N адресов → 32-байт word с byte-флагами; параллельная валидация не нужна — линейно); energy по формуле;
  - 0x0a ValidateMultiSign: локально permission-стейта нет — вернуть 0 (false) с комментарием-обоснованием ИЛИ честный revert-стаб — решить по java-tron поведению на несуществующем аккаунте (проверить в Step 1);
  - shielded 0x1000001-4 и vote/freeze 0x1000005-0x1000015: `new_stateful` стабы → revert "tron precompile X not supported in local tests (stage 2 stub)" с корректной energy;
  - все чистые — `DynPrecompile::new` (кэшируемые), стейтовые/стабы — `new_stateful`; ошибки входа — halt/revert, НЕ PrecompileError.
  - Инжекция в `inject_tron_extensions` через `extend_precompiles` (перекрывает blake2f@0x09 и KZG@0x0a — намеренно, покрыто контраст-тестом).
- [ ] **Step 4: Labels:** `TRON_PRECOMPILES: &[(&str, Address)]` в evm/networks (зеркало TEMPO_PRECOMPILES), ветки is_tron в `precompiles_label`/`precompiles`. Трейсы forge показывают имена.
- [ ] **Step 5:** тесты зелёные (`cargo test -p foundry-evm-core tron`), sandbox 4/4, clippy/fmt, commit `feat(tron): java-tron precompile set for tron-revm`.

---

### Task 3: CREATE2 по формуле Tron + cheatcodes

**Files:**
- Create: `crates/evm/core/src/evm/tron/create.rs` (инструкция 0xF5 + формулы)
- Modify: `tron/mod.rs` (insert_instruction 0xF5), `crates/tron/primitives/src/address.rs` (+`create2_address(sender, salt, init_code_hash)` — переиспользуемая формула), `crates/cheatcodes/src/utils.rs` или соотв. диспатч (`computeCreate2Address` для Tron)
- Sandbox: `sandbox/tron-counter/test/` (+create2-тест), возможно `src/` (+factory)

**Steps:**

- [ ] **Step 1: Формула в primitives + падающий юнит.** `foundry_tron_primitives::address::create2_address(sender: Address, salt: B256, init_code_hash: B256) -> Address` = `keccak256(0x41‖sender20 ‖ salt ‖ init_code_hash)[12..]`. Вектор: синтетический + подтверждение формулы по `WalletUtil.generateContractAddress2` (верификатор обязан перечитать источник). Плюс live-вектор (Step 5) станет вторым ассертом.
- [ ] **Step 2: Инструкция 0xF5.** Копия стоковой `create::<true>` из revm-interpreter contract.rs с двумя изменениями: (a) НЕ применять EIP-3860 initcode-метринг (у Tron его нет; gas CREATE2 = 32000 + 6/word кода + память — по EnergyCost `getCreate2Cost`), (b) `CreateInputs` со `scheme = CreateScheme::Custom { addr }`, где addr по формуле Step 1 (sender = `interpreter.input.target_address()`). Вставить через `insert_instruction(0xF5, ..., 0)` (динамический газ внутри — сверить, как стоковая create метрит: если газ в инструкции, static=0). ВНИМАНИЕ на консистентность с cheatcode-инспектором (Custom-scheme читается им же — по разведке OK).
- [ ] **Step 3: Cheatcode.** `vm.computeCreate2Address(salt, initCodeHash[, deployer])` для Tron-сети → формула Tron. Найти диспатч-шов (utils generic over FEN или runtime-проверка networks.is_tron()) — минимальный дифф. `computeCreateAddress` НЕ менять (локальный CREATE остаётся EVM) — задокументировать в doc-comment.
- [ ] **Step 4: Sandbox-тест** (оффлайн): factory-контракт с `new Counter{salt: s}()`, тест ассертит фактический адрес развёрнутого == `vm.computeCreate2Address(...)` == адрес по локальному вычислению в Solidity (keccak256(abi.encodePacked(bytes1(0x41), address(factory), s, keccak256(initcode)))) — тройная согласованность, нетавтологично (три независимых пути). forge test 5/5 теперь.
- [ ] **Step 5: Live golden CREATE2:** деплой factory на Nile (`deploy_contract`), вызов create2-метода (`trigger_contract`), чтение фактического адреса потомка (событие или геттер) → ассерт == локальная формула. Гейт TRON_LIVE, экономно (один деплой + один вызов).
- [ ] **Step 6:** тесты/clippy/fmt/commit `feat(tron): tron CREATE2 address scheme and cheatcode`.

---

### Task 4: Golden energy + документация + STATUS

**Files:**
- Modify: `crates/tron/provider/src/client.rs` или новый тест-файл (golden live-тест)
- Modify: `docs/tron/STATUS.md`, `docs/tron/specs/2026-07-10-foundry-tron-fork-design.md` (фактические поправки с пометкой)

**Steps:**

- [ ] **Step 1: Golden energy live-тест** (TRON_LIVE): (a) view-путь — `trigger_constant` на fresh-задеплоенном Counter (`number()`), сравнить `energy_used` ноды с локальной симуляцией того же вызова через TronEvmFactory (тот же runtime-код в CacheDB) — точное равенство; (b) write-путь — `setNumber(7)`: локальная симуляция vs `TxInfo.energy_used` подтверждённой транзакции. При расхождении: разобраться и починить модель (Task 1) — НЕ ослаблять ассерт допусками без анализа; допуск возможен только с задокументированной причиной (напр. dynamic energy penalty — но fresh контракт имеет factor=1).
- [ ] **Step 2: Спека — фактические поправки** (аккуратной правкой с пометкой «уточнено 2026-07-12 по java-tron/live»): energyFee 420 → 100 SUN; «energy = revm-gas 1:1» → «FRONTIER-таблицы + TVM-дельты (план E)»; отметить формулу CREATE2 и отсутствие EIP-170/3860.
- [ ] **Step 3: STATUS.md**: план E ✅ с перечнем (energy-модель, precompiles, CREATE2), новые находки (0x03 ≠ ripemd160; BatchValidateSign формат; refund-факт из Task 1; depth-64 сделан/отложен; jsdelivr-канал для java-tron; nileex vs trongrid), следующие планы Этапа 2 (F: резолвер tron-solc; G: fork-режим через /jsonrpc с nonce-шимом — блокер eth_getTransactionCount -32601 постоянный, решение tower-layer).
- [ ] **Step 4:** полный критерий: `cargo test -p foundry-evm-core tron` всё зелёное; sandbox forge test 5/5; `cargo check --workspace` чистый; live golden прогнан. Commit'ы: `test(tron): golden energy parity against nile` + `docs(tron): stage-2 core done, spec corrections`.

---

## Вне скоупа плана E (зафиксировать в STATUS, не делать)

- Резолвер tron-solc (план F: крейт foundry-tron-solc, GitHub-релизы + solc-bin чексуммы, шов Config::ensure_solc; svm непригоден — hardcoded soliditylang URL);
- Fork-режим (план G: tower-shim eth_getTransactionCount→0x0 на tron-пути; остальное уже работает по разведке);
- Внутренний CREATE по Tron-схеме (rootTxId+глобальный op-счётчик — невоспроизводимо локально; документированная дельта);
- Точная эмуляция DynamicEnergy penalty (нондетерминизм по популярности контракта — golden-тесты используют fresh контракты);
- MEM_LIMIT 3MB / bandwidth-колонка в gas-report / freeze-vote опкоды 0xD5-0xDF честные (стабы есть с этапа 1.5? — 0xD5+ вообще не введены: не эмитятся компилятором, отложить);
- energy-report UI (заголовки таблиц gas-report) — вместе с bandwidth в плане F/G.

## Критерий завершения плана E

`cargo test -p foundry-evm-core tron` — все зелёные (энергия/precompiles/create2 + старые); sandbox `forge test` 5/5 на tron-solc байткоде; golden live: energy exact-match на view и write, CREATE2-адрес подтверждён on-chain; `cargo check --workspace` чистый; Ethereum/Tempo/Op-тесты не задеты; спека и STATUS.md обновлены.
