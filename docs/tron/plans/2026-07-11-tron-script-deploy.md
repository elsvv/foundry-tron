# Tron Deploy Pipeline Plan (План D — cast/forge script/forge create → Nile)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** полный цикл Этапа 1 спеки (§6): `forge build` (tron-solc) → `forge test` (tron-revm) → **деплой и вызовы на Nile** через `cast send`, `forge script --broadcast`, `forge create` — protobuf-путём (`/wallet/*`), без eth_sendRawTransaction. Закрывает пункт «forge script/деплой на Nile» Этапа 1.

**Спека:** `docs/tron/specs/2026-07-10-foundry-tron-fork-design.md` §4.5–4.7. Разведка: scout-plan-d (6 отчётов, сессия 2026-07-11); все критичные доменные факты ниже перепроверены по первоисточникам оркестратором.

## Верифицированные доменные факты (НЕ пересматривать без первоисточника)

1. **Адрес контракта из txID** — java-tron `chainbase/.../WalletUtil.java:39-51` (@develop, проверено 2026-07-11):
   ```java
   byte[] txRawDataHash = trxCap.getTransactionId().getBytes();      // txID = sha256(raw_data), 32 байта
   byte[] combined = txRawDataHash ++ ownerAddress;                   // owner — 21 байт, с префиксом 0x41
   return Hash.sha3omit12(combined);                                  // keccak256(combined)[12..32]
   ```
   Итог: `contract_address(20B) = keccak256(txid32 ‖ owner21)[12..]`. НЕ «keccak(txID)», НЕ RLP — оба варианта из разведки были неверны; формула выше — из исходника. Верификатор обязан сверить формулу с реальной deploy-транзакцией сети (txID+owner → фактический contract_address из `gettransactioninfobyid`).
2. **Chain id:** mainnet `728126428` (0x2b6653dc), **Nile `3448148188` (0xcd8690dc)** — проверено live через `POST <host>/jsonrpc eth_chainId` 2026-07-11. Nile id нужен для путей `broadcast/<script>/<chain>/`.
3. **Подпись:** txID = sha256(raw_data); 65 байт `r‖s‖(27+recid)`; никакого EIP-155/chain id в подписи (replay-защита — TAPOS+expiration). `foundry_wallets::WalletSigner` реализует ТОЛЬКО async `alloy_signer::Signer::sign_hash` (нет `SignerSync`) — интеграция обязана быть async. `foundry-wallets` — внешний git-dep (foundry-core), патчить его нельзя.
4. **Broadcast:** остаёмся на `/wallet/broadcasthex` (работает, покрыт live-тестами; `/wallet/broadcasttransaction` из спеки — эквивалент, миграция не нужна; отметить в STATUS.md как уточнение спеки).
5. **fee_limit** — поле `TransactionRaw.fee_limit` (tag 18, i64, единицы SUN); `origin_energy_limit`/`consume_user_resource_percent` — поля `SmartContract` (tags 8/6). Это два разных уровня протобуфа.
6. **TronGrid `/jsonrpc`** (тот же хост, что `/wallet/*`) отдаёт частичный eth_* JSON-RPC — read-путь по спеке §4.5. Для записи НЕ существует.

## Global Constraints

- Репо: `/Users/andrey/vibe_projects/foundry-tron`, ветка **`tron-dev-continue`**. Коммитить локально, НЕ пушить (оркестратор пушит и обновляет PR #1).
- Тулчейн: `export PATH="$HOME/.cargo/bin:$PATH"` (свежий rustup, stable 1.97); формат — `cargo +nightly fmt`. Долгие сборки: timeout 600000, при таймауте перезапускать (инкрементально).
- Live-тесты: `.env.tron-dev` в корне репо (`TRON_PRIVATE_KEY`, НЕ коммитить), адрес `TX7izXWcmofRYonzdcThrS78jifMtVWCuf`, ~1997 TRX на Nile. Гейт: `TRON_LIVE=1` + явный `eprintln!("skipped…")` без переменной. Live-деплои: контракты маленькие (Counter), fee_limit для тестов ≤ 400_000_000 SUN (400 TRX); реальный расход на деплой Counter — единицы TRX.
- Тесты: только реальные векторы (сеть, официальные .proto, java-tron); никакой тавтологии; запрещено ослаблять ассерты и `#[ignore]`.
- Минимальные диффы в существующих файлах; вся Tron-логика — в tron-крейтах и отдельных `run_tron`/`broadcast_tron`-путях (риск rebase из спеки §8). Никаких новых внешних зависимостей (внутри-workspace path-deps — можно).
- tron-крейты остаются dependency-light: `foundry-tron-primitives` НЕ тянет foundry-config/foundry-wallets; generic `impl alloy_signer::Signer`-подпись — да, конкретные типы кошельков — нет.
- Комментарии кода — английский, конвенции — CLAUDE.md репо. Коммиты conventional c `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Точные номера строк — из разведки 2026-07-11; при сдвиге ориентироваться на именованные структуры/функции.

## Architecture (решения, следуют спеке §4.5)

- **Tempo-паттерн НЕ переносится**: Tempo меняет envelope, но шлёт через `eth_sendRawTransaction`; Tron шлёт protobuf через `/wallet/broadcasthex`. Поэтому: НЕ делаем `TronNetwork: alloy::Network`; `TronEvmNetwork::Network = Ethereum` остаётся; обходим alloy-send целиком отдельными inherent-путями (прецедент: `impl BundledState<TempoEvmNetwork>::broadcast_batch`, broadcast.rs:912).
- **Диспетчеризация — только явная** (`network = "tron"` в foundry.toml / `--network tron`): `From<ChainId>` не мапит Tron, `infer_network_from_fork` не работает с `/wallet/*` — не полагаться на инференс нигде.
- **forge script**: `prepare_bundled::<TronEvmNetwork>` реюзается (симуляция уже на TronEvmFactory); Tron-ветка в `run_script` до Eth-фолбэка; отдельный `broadcast_tron`; фаза-2 fork-симуляции для Tron пропускается (форк требует eth_ JSON-RPC); nonce-запросы short-circuit.
- **Адрес деплоя**: в симуляции EVM-схема даёт неверный адрес — истинный вычисляется локально по формуле факта №1 сразу после сборки raw_data и **обязательно сверяется** с `TxInfo.contract_address` после подтверждения (mismatch = hard error).
- **Артефакты broadcast/*.json**: формат тот же; `hash` несёт txID (B256 — совместимо); новые Tron-поля — опциональная секция `tron: Option<TronTxMeta>` с `#[serde(default, skip_serializing_if = "Option::is_none")]` (EVM-фикстуры не ломаются).

---

### Task 1: tron-primitives/provider — фундамент деплоя (билдеры, async-подпись, адрес из txID)

**Files:**
- Modify: `crates/tron/primitives/src/address.rs` (+`contract_address_from_txid`)
- Modify: `crates/tron/primitives/src/sign.rs` (+async `sign_raw_with`)
- Modify: `crates/tron/primitives/src/lib.rs` (реэкспорты: `CreateSmartContract`, `SmartContract`, `TransferContract`, `type_url`, новые фн)
- Modify: `crates/tron/provider/src/client.rs` (+`build_trigger_raw`, `build_create_raw` (pub), +`TxOptions{fee_limit, expiration_ms}`, +`deploy_contract`, +`trigger_contract`, конфигурируемый поллинг)
- Create: `crates/tron/primitives/testdata/nile_create_tx.json` (реальная deploy-транзакция Nile: txID, raw_data_hex, owner, фактический contract_address)

**Steps:**

- [ ] **Step 1: Реальный вектор CreateSmartContract.** Найти на Nile недавнюю deploy-транзакцию (`/wallet/gettransactionbyid` + `gettransactioninfobyid`; можно взять любой свежий деплой из блока или задеплоить вручную через `primitives/examples/nile_smoke.rs`-подобный однократный вызов). Зафиксировать в `testdata/nile_create_tx.json`: `txID`, `raw_data_hex`, `owner_address` (hex41), `contract_address` (hex41). Требование: это НАСТОЯЩИЙ ответ ноды (сохранить как есть).
- [ ] **Step 2: Падающие тесты.**
  - `address::contract_address_from_txid(txid: B256, owner: Address) -> Address`: тест на векторе Step 1 — формула из факта №1 даёт фактический `contract_address`.
  - Round-trip `CreateSmartContract`: декодировать `raw_data_hex` вектора в `TransactionRaw`, достать `Contract.parameter` → `CreateSmartContract`, проверить `txid(raw) == txID` вектора, перекодировать — байт-в-байт.
  - `sign_raw_with`: async-тест — подпись тем же ключом, что `sign_raw`, даёт идентичные 65 байт (детерминизм secp256k1 RFC6979) и `recover_address_from_prehash` совпадает.
- [ ] **Step 3: Реализация primitives.**
  - `contract_address_from_txid`: `keccak256`-хелпер уже в `alloy_primitives`; owner расширить до 21 байта `0x41‖owner`.
  - `pub async fn sign_raw_with<S: alloy_signer::Signer + ?Sized>(raw: TransactionRaw, signer: &S) -> alloy_signer::Result<SignedTronTx>` — тот же 65-байтовый пакинг, что `sign_raw` (вынести общий пакинг в приватный хелпер; `sign_raw` не ломать — им пользуются live-тесты).
- [ ] **Step 4: Реализация provider.**
  - `pub struct TxOptions { pub fee_limit: i64, pub expiration_ms: i64 }` (+`Default`: 1_000_000_000 / 60_000).
  - `pub fn build_trigger_raw(owner, contract, call_value, data, rb: RefBlock, now_ms, opts: &TxOptions) -> TransactionRaw` и `pub fn build_create_raw(owner, bytecode ++ ctor_args, name, call_value, origin_energy_limit, consume_user_resource_percent, rb, now_ms, opts) -> TransactionRaw` — по образцу `build_transfer_raw` (21-байтовые адреса, TAPOS, expiration из opts, `fee_limit` на raw_data). `build_transfer_raw` тоже перевести на `TxOptions` (это уберёт хардкод 60с; live-тесты не трогать по ассертам).
  - `pub async fn deploy_contract<S: Signer + ?Sized>(&self, signer: &S, bytecode_with_args: Vec<u8>, name: &str, opts: &TxOptions, poll: (u32, Duration)) -> Result<(B256, Address, TxInfo)>`: tapos → build_create_raw → sign_raw_with → **вычислить адрес локально** → broadcast → wait_for_confirmation → **assert локальный адрес == TxInfo.contract_address** (mismatch = `TronError::Decode`-класс ошибка с обоими значениями) → вернуть (txid, addr, info).
  - `pub async fn trigger_contract<S>(&self, signer, contract: Address, call_value, data, opts, poll) -> Result<(B256, TxInfo)>` — аналогично.
- [ ] **Step 5: Тесты зелёные.** `cargo test -p foundry-tron-primitives -p foundry-tron-provider` — все оффлайн; live: `TRON_LIVE=1` (+ключ) — новый live-тест `live_deploy_counter_on_nile`: деплой МИНИМАЛЬНОГО контракта (creation-байткод Counter из `crates/evm/core/testdata/tron_counter_creation.hex` — уже есть fixture!) c fee_limit 400 TRX, сверка адреса, затем `trigger_contract(setNumber(7))` + `trigger_constant(number()) == 7`. Явный skip-eprintln без TRON_LIVE.
- [ ] **Step 6: clippy/fmt/commit** — `feat(tron): contract deploy and trigger via protobuf provider`.

---

### Task 2: config — секция [tron] + rpc-алиасы + CLI-опции

**Files:**
- Create: `crates/config/src/tron.rs` (`TronConfig`)
- Modify: `crates/config/src/lib.rs` (`pub tron: TronConfig` + serde default + доки)
- Modify: `crates/config/src/endpoints.rs` (builtin алиасы `tron`, `nile`, `shasta` → TronGrid-хосты из спеки §4.7)
- Modify: `crates/cli/src/opts/` (+`tron.rs`: `TronOpts { fee_limit: Option<i64>, tron_expiration: Option<u64> }`, флаги `--tron.fee-limit`, `--tron.expiration`; flatten туда, где понадобится в Task 3/4/5)

**Steps:**

- [ ] **Step 1: Падающий тест конфига.** В `crates/config` тест: foundry.toml с `network = "tron"` + `[tron] fee_limit = 5`, `expiration = 30` парсится; дефолты по спеке §4.7: `fee_limit = 1_000_000_000`, `origin_energy_limit = 10_000_000`, `user_fee_percentage = 100`, `expiration = 60`. Тест на `Config::default()` — секция присутствует с дефолтами и round-trip'ится (`to_string_pretty`).
- [ ] **Step 2: `TronConfig`** — struct с полями выше (`u64`/`i64` по месту использования протобуфа; expiration в секундах, конвертация в ms — на потребителе). Doc-comments на каждом поле со ссылкой на семантику (fee_limit — SUN, cap на burn за tx; user_fee_percentage 0-100). `impl TronConfig { pub fn tx_options(&self) -> foundry_tron_provider::TxOptions }`? — НЕТ: config не должен зависеть от tron-provider; конвертацию делает потребитель (script/cast). Держать чистым.
- [ ] **Step 3: rpc-алиасы** в `endpoints.rs` рядом с tempo: `"tron" → https://api.trongrid.io`, `"nile" → https://nile.trongrid.io`, `"shasta" → https://api.shasta.trongrid.io`.
- [ ] **Step 4: `TronOpts`** по образцу `TempoOpts` (минимум: два флага выше; `fn apply(&self, cfg: &TronConfig) -> TronConfig` — CLI перекрывает конфиг).
- [ ] **Step 5:** `cargo test -p foundry-config`, clippy, fmt, commit — `feat(tron): [tron] config section, rpc aliases and CLI opts`.

---

### Task 3: cast — to-sun/from-sun/tron-address, call/balance/send через tron-provider

**Files:**
- Modify: `crates/cast/Cargo.toml` (+foundry-tron-primitives, +foundry-tron-provider; добавить оба в `[workspace.dependencies]` корневого Cargo.toml)
- Modify: `crates/cast/src/opts.rs` (+`ToSun`/`FromSun`/`TronAddress` варианты CastSubcommand)
- Modify: `crates/cast/src/args.rs` (match-арки утилит)
- Modify: `crates/cast/src/lib.rs` (`SimpleCast::to_sun/from_sun` — делегация в `foundry_tron_primitives::units`; в units добавить обратный парс `parse_trx_to_sun(&str)`)
- Modify: `crates/cast/src/cmd/call.rs`, `send.rs` (+tron-ветки), возможно `crates/cast/src/tron.rs` (новый модуль общих хелперов: провайдер из config, парс адресов, вывод)
- Modify: `sandbox/tron-counter/README.md` (команды примеров)

**Steps:**

- [ ] **Step 1: Оффлайн-утилиты (падающие тесты сначала, casttest!/юнит):**
  - `cast to-sun 1.5` → `1500000`; `cast from-sun 1500000` → `1.500000` (шаблон — ToWei/FromWei: opts.rs:311-337, args.rs:164-172).
  - `cast tron-address T…|41…|0x…` → печатает все три формы (base58, hex41, 0x); ошибки на кривой чексумме. Использует `foundry_tron_primitives::address`.
- [ ] **Step 2: `cast call` (constant):** в `CallArgs::run` (call.rs:243) — после существующих ранних веток: `config.networks.is_tron()` → собрать calldata (тот же encode-путь, что дальше по функции — sig+args, без провайдера), owner = `--from` либо нулевой, `TronProvider::trigger_constant`, вывод hex результата (как обычный call). `--trace`/fork-опции с Tron — явная ошибка «not supported on tron yet».
- [ ] **Step 3: `cast balance`:** в args.rs Balance-арке: `config.networks.is_tron()` → `TronProvider::get_balance`; `--ether` для Tron печатает TRX через `format_sun_as_trx` (6 знаков), НЕ from_wei.
- [ ] **Step 4: `cast send`:** в `SendTxArgs::run` (send.rs:105) ДО `run_generic`: `config.networks.is_tron()` → `run_tron()`:
  - резолв signer'а: `wallet.signer().await?` → `WalletSigner` (async sign_hash — работает для всех бэкендов);
  - `to` парсится через tron-primitives (`T…`/`41…`/`0x…`);
  - без calldata и с `--value` → TransferContract (send_transfer-путь, но через WalletSigner: собрать raw через `build_transfer_raw` + `sign_raw_with`);
  - с calldata → `trigger_contract`; `--create <bytecode> [ctor-args]` → `deploy_contract`;
  - fee_limit/expiration: `config.tron` + `TronOpts` override;
  - вывод: txID, статус, energy_used, fee (TRX), для деплоя — адрес в base58 И hex41. stdout — машинно-читаемый результат (txID или адрес), прочее — stderr (sh_* макросы).
- [ ] **Step 5: Live-гейт (TRON_LIVE=1, cli-тест или ручной прогон с фиксацией вывода):** `cast send <получатель> --value 1000000` (1 TRX перевод на ДРУГОЙ адрес, НЕ самому себе — Tron отклоняет self-transfer на валидации контракта: `CONTRACT_VALIDATE_ERROR: Cannot transfer TRX to yourself`) на Nile — txID подтверждён; `cast balance` совпадает с `/wallet/getaccount`; `cast call number()` на контракте из Task 1 live-теста. Зафиксировать команды в sandbox/README.
- [ ] **Step 6:** `cargo test -p cast@1.7.2`, clippy, fmt, commit — `feat(tron): cast send/call/balance and sun/address utilities`.

---

### Task 4: forge script — Tron-диспетчеризация, broadcast_tron, артефакты

**Files:**
- Modify: `crates/script/Cargo.toml` (+tron-крейты)
- Modify: `crates/script/src/lib.rs` (is_tron выбор networks в `resolved_evm_opts` — НЕ вызывать `infer_network_from_fork` при config-Tron; Tron-ветка в `run_script` до Eth-фолбэка; `sender_nonce` short-circuit при is_tron в `ScriptConfig::new`/`update_sender`)
- Create: `crates/script/src/tron.rs` (`impl BundledState<TronEvmNetwork> { async fn broadcast_tron(self) -> Result<BroadcastedState<TronEvmNetwork>> }` + хелперы)
- Modify: `crates/script/src/simulate.rs` / `execute.rs` (skip fork-фазы и `check_shanghai_support` для Tron; contract_address при create для Tron НЕ заполнять EVM-схемой — оставить None/пометить)
- Modify: `crates/script-sequence/src/transaction.rs` (+`pub tron: Option<TronTxMeta>` с serde(default); `TronTxMeta { txid: B256, owner_base58: String, contract_address_base58: Option<String>, fee_limit: i64, energy_used: Option<u64>, fee_sun: Option<u64> }` — сам тип можно объявить в script-sequence без зависимостей от tron-крейтов, base58-строки готовит script)
- Modify: `sandbox/tron-counter/` (+`script/Deploy.s.sol`)
- Modify: `docs/tron/STATUS.md` (после Task 5 — финально; здесь не трогать)

**Steps:**

- [ ] **Step 1: Санити-тест диспетчеризации (оффлайн).** forgetest-style или юнит: `network = "tron"` в конфиге → `run_script` уходит в Tron-ветку (не в generic), `prepare_bundled::<TronEvmNetwork>` собирает транзакции локальной симуляцией без fork-url и БЕЗ обращений к eth_* (запуск без сети должен доходить до dry-run вывода). `--fork-url` с Tron → явная ошибка «forking a Tron node is not supported (stage 2)».
- [ ] **Step 2: `broadcast_tron`.** Sequential, batch=1, по каждой tx из sequence: классифицировать (create — `to == None` / call / transfer по calldata+value) → TAPOS свежий на КАЖДУЮ tx → build_*_raw (fee_limit/expiration из `config.tron`+TronOpts; `origin_energy_limit`/`user_fee_percentage` для create) → `sign_raw_with(WalletSigner)` → локальный адрес (для create) → broadcast → `wait_for_confirmation` (attempts/interval согласованы с expiration: attempts*interval ≥ expiration+30s) → `add_pending`/receipts:
  - hash = txID; TronTxMeta заполнить; contract_address в metadata — перезаписать честным Tron-адресом; сверка с TxInfo.contract_address — mismatch = ошибка;
  - receipts: собрать `TransactionReceipt` (N=Ethereum) из TxInfo: status=success, gas_used=energy_used, contract_address, block_number; effective_gas_price=0 — и Tron-числа (fee_sun) печатать в прогресс-выводе отдельно (TRX, не ETH);
  - ретраи: transient HTTP — до 3 с бэкоффом; **истёкший expiration — пересборка raw с новым TAPOS и переподпись (новый txID), максимум 2 пересборки** (спека §4.5/§8);
  - `--resume`: реюз `remaining_transaction_start` (индекс по receipts.len); незавершённые pending с истёкшим expiration — пересborка (см. выше).
- [ ] **Step 3: Гейт симуляции.** `prepare_bundled` для Tron: пропустить fork-симуляцию фазы 2 (путь skip_simulation) и Shanghai-чек; библиотеки-предеплои (`ScriptPredeployLibraries`) в MVP: если скрипт требует линковки библиотек — явная ошибка «library predeploys on tron: stage 2» (зафиксировать; Counter их не требует).
- [ ] **Step 4: `Deploy.s.sol`** в sandbox: `new Counter()` + `counter.setNumber(42)` под `vm.startBroadcast()`. Без forge-std (как остальной sandbox; мини-интерфейс Vm с broadcast-читкодами уже доступен через встроенный cheatcode-интерфейс — по образцу существующего Counter.t.sol).
- [ ] **Step 5: E2E live-гейт (TRON_LIVE=1):**
  ```bash
  cd sandbox/tron-counter
  ../../target/debug/forge script script/Deploy.s.sol --rpc-url nile --broadcast --private-key $TRON_PRIVATE_KEY
  ```
  Ожидание: деплой подтверждён на Nile; `broadcast/Deploy.s.sol/3448148188/run-latest.json` существует, содержит txID (hash), `tron.contract_address_base58`; адрес контракта отвечает `cast call <addr> "number()" --rpc-url nile` → 42 (cast из Task 3). Оффлайн-CI-вариант: без TRON_LIVE тест только dry-run (Step 1).
- [ ] **Step 6:** `cargo test -p forge-script -p forge-script-sequence` (+существующие тесты script-sequence НЕ ломаются — serde(default)), clippy, fmt, commit — `feat(tron): forge script broadcast via tron protobuf path`.

---

### Task 5: forge create + финальный сквозной гейт + STATUS.md

**Files:**
- Modify: `crates/forge/Cargo.toml` (+tron-крейты), `crates/forge/src/cmd/create.rs` (tron-ветка до alloy-провайдера)
- Modify: `docs/tron/STATUS.md`
- Modify: `sandbox/tron-counter/README.md` (полный цикл команд)

**Steps:**

- [ ] **Step 1: `forge create` Tron-ветка.** В `CreateArgs::run` (create.rs:135) ПЕРВЫМ делом (до какого-либо alloy-провайдера/`get_chain_id`): загрузить config; `config.networks.is_tron()` → `run_tron()`: компиляция/линковка как в run_generic до сборки tx; bytecode+ctor_args → `deploy_contract` (config.tron + TronOpts); вывод: Deployer (base58), Deployed to (base58 + hex41), txID. `--verify`/`--unlocked`/browser с Tron — явные ошибки «not supported on tron yet».
- [ ] **Step 2: Live-гейт create:** `forge create src/Counter.sol:Counter --rpc-url nile --private-key ...` из sandbox — контракт на Nile, `cast call number()` → 41 (initial из конструктора… у Counter конструктора нет — тогда `setNumber` через `cast send` → повторный call = 7). Дописать README-цикл.
- [ ] **Step 3: Полный сквозной прогон Этапа 1 (руками, зафиксировать выводы в отчёте):** build (tron-solc) → test (4/4) → create → cast send → cast call → forge script --broadcast. Всё на одном sandbox-проекте против Nile.
- [ ] **Step 4: STATUS.md:** план D ✅ (что именно работает: cast send/call/balance/to-sun/from-sun/tron-address, forge script --broadcast, forge create; live-подтверждения — txID'ы); уточнение спеки про broadcasthex; новые находки (формула адреса с цитатой java-tron, Nile chain id 3448148188); Этап 2 — следующий; таблица этапов.
- [ ] **Step 5:** `cargo check --workspace` чистый; clippy tron-затронутых крейтов; fmt; commit — `feat(tron): forge create on tron and stage-1 e2e` + `docs(tron): mark plan D done`.

---

## Вне скоупа плана D (зафиксировать, не делать)

- `cast block/logs/storage/tx/receipt` через tron-provider (частично работают через `/jsonrpc` — Этап 2);
- library predeploys в forge script на Tron; CREATE2 (0x41-префикс, `generateContractAddress2`) — Этап 2;
- energy→fee_limit автооценка через `estimateenergy` (в D fee_limit из конфига); gas-report в TRX — Этап 2;
- `--batch`, browser/unlocked-подпись, hardware-специфика (архитектурно поддержана через async sign_hash, live-проверка — по возможности);
- согласование латентных «наивных» арок (`args.rs:953` DecodeTransaction, `da_estimate.rs:43`) — задокументировать в STATUS.md как известное упрощение.

## Критерий завершения плана D

`cargo test -p foundry-tron-primitives -p foundry-tron-provider -p foundry-config` зелёные (оффлайн); `cargo check --workspace` чистый; live-цепочка на Nile воспроизведена: деплой через provider-тест, `cast send`+`call`+`balance`, `forge script --broadcast` (артефакт с txID и base58-адресом), `forge create`; STATUS.md обновлён. После этого — Этап 2 (полный tron-revm: precompiles, CREATE2, energy-report, резолвер tron-solc).
