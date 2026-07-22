# План I — Этап 4 «fidelity»: TIP-491, precompile-кламп, transfer-баг, chain-params, base58, tron-solc 0.8.28+

Дата: 2026-07-23. Статус: утверждён к исполнению (dynamic workflow: Opus-исполнители + Opus-верификация каждой задачи + Fable-ревью каждые 2 задачи; ветка `tron-stage4` от `master`).

Закрывает подтверждённые находки fidelity-аудита 2026-07-23 (45 агентов, адверсариальная
верификация; полный JSON находок — в session-scratchpad, ключевые факты продублированы ниже,
план самодостаточен). Цель этапа: mainnet-оценкам энергии/стоимости можно доверять для
«горячих» контрактов (USDT-класс), деплой/سend-пути не отдают ноде отвергаемые транзакции,
локальная precompile-таблица не содержит эфирных адресов, которых нет в java-tron, адреса
в трейсах — base58, новые версии tron-solc подхватываются без релиза тулчейна.

> **For agentic workers:** исполнять задача-за-задачей (subagent-driven). Каждая задача —
> отдельный тест-цикл и отдельный коммит(ы); чекбоксы — прогресс. Верификаторы обязаны
> перепроверять доменные факты по первоисточникам (java-tron @develop, live-ноды).

---

## Проверенные факты (основания)

### F-1. TIP-491 dynamic energy — механика и mainnet-параметры (live, 2026-07)

- Формула: `charged = base × (1 + energy_factor/10000)`; фактор применяется к энергии,
  исполненной «в контексте контракта» (его собственные инструкции, не субвызовы), и к
  прямым триггерам, и к internal calls. Активен с Proposal #83 (2023-02-05).
  Источник: `tips/tip-491.md`, `tips/issues/508`.
- Mainnet-параметры (getchainparameters, снято 2026-07): `getDynamicEnergyThreshold =
  5_000_000_000`, `getDynamicEnergyIncreaseFactor = 2000` (20%), decrease = increase/4,
  `getDynamicEnergyMaxFactor = 34000` (макс. 3.4 → до 4.4× total). Обновление фактора —
  раз в maintenance-цикл (6ч).
- USDT `TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t` стоит на максимуме `energy_factor = 34000`;
  `transfer()` существующему держателю ~64.9k energy (≈14.75k base × 4.4), свежему ~130k.
  Никакого отдельного «USDT tx-типа» не существует — это именно TIP-491.
- API: `energy_used` из `/wallet/triggerconstantcontract` **уже включает** penalty;
  `energy_penalty` (4.7.2+) — доля penalty; `base = energy_used − energy_penalty`.
  Receipt: `energy_usage_total`, `energy_penalty_total`, `energy_usage` (caller),
  `origin_energy_usage`, `net_usage`, `net_fee`. Per-contract фактор:
  `/wallet/getcontractinfo` → `contract_state.energy_factor` (precision 10000).
- `/wallet/estimateenergy` (4.7.0.1+) выключен по умолчанию на нодах (нужны
  `vm.estimateEnergy` + `vm.supportConstant`); обязателен fallback на
  triggerconstantcontract.

### F-2. Экономика (live, 2026-07)

- Цена энергии `getEnergyFee` = **100 sun** (история: 420 → 210 (Proposal 95) → 100
  (Proposal 104, 2025-08-29)). Наш `TRON_ENERGY_FEE_SUN = 100` (`energy.rs:36`) актуален,
  но захардкожен.
- `getMaxFeeLimit` = 15_000_000_000 sun (15 000 TRX) — нода отвергает fee_limit выше.
- Bandwidth burn = 1000 sun/байт; memo fee = 1 TRX (`getMemoFee`); free bandwidth 600/день.
- Рекомендованный fee_limit: `energy_estimate × price × буфер (~1.2)`, clamp 15 000 TRX;
  worst-case: `base × (1 + max_factor) × price`.
- `allowTvmOsaka` = 0 на mainnet и Nile (проба 2026-07-12). Nile активирует апгрейды
  РАНЬШЕ mainnet — параметры двух сетей могут расходиться в любой момент.

### F-3. Precompile-леак (подтверждён аудитом)

`inject_tron_extensions` (`crates/evm/core/src/evm/tron/mod.rs:140`) делает
`extend_precompiles(tron_precompiles())` ПОВЕРХ базовой карты revm, которая строится из
спека конфига (дефолт `evm_version = Osaka`). Поэтому эфирные BLS12-381 (0x0b–0x11) и
P256Verify (0x100) остаются активны, хотя на Tron их нет (докстрока `precompiles.rs:69-70`
«P256 intentionally absent» описывает намерение, не реальность). `tron_precompiles()`
(`precompiles.rs:71-103`) уже содержит ПОЛНЫЙ набор java-tron: faithful 0x01–0x08
(включая двойной sha256 на 0x03 и EIP-198 ModExp на 0x05), TIP-43/60 на 0x09/0x0a,
0x020003/0x020009, 21 стаб (`STUBS`, `precompiles.rs:107-129`). Т.е. верная таблица =
**ровно** `tron_precompiles()`, без эфирной базы. Вызов отсутствующего precompile-адреса
в java-tron — обычный вызов пустого аккаунта (success, пустой выход).

### F-4. Transfer-баг (подтверждён аудитом, confirmed)

- `crates/cast/src/cmd/send.rs:218-223` (`run_tron`): `if data.is_empty() {
  build_transfer_raw(...) } else { build_trigger_raw(...) }` — решение чисто по calldata,
  без проверки «контракт ли dest». `cast send <контракт> --value N` без сигнатуры строит
  `TransferContract`, который java-tron отвергает для контракт-адресов.
- `crates/script/src/tron.rs:258-287` (`fn classify`, enum `TronCall` :60-69): та же
  логика — `Some(to)` + пустой input → `TronCall::Transfer`.
- В провайдере (`crates/tron/provider/src/client.rs`) нет НИКАКОЙ проверки на контракт:
  `get_balance` читает только `balance`. Пути добавления: POST `/wallet/getcontract`
  (для обычного аккаунта возвращает `{}`), либо код через `/jsonrpc` eth_getCode.
- Семантика java-tron: `TriggerSmartContract` с пустым `data` и `call_value > 0` — легальный
  вызов payable fallback/receive.

### F-5. Провайдер/конфиг/gas-report — точные швы (скаут 2026-07-23)

- `TxOptions` (`client.rs:27-37`): `fee_limit: i64` (default 1_000_000_000), `expiration_ms`.
  `wrap_raw` (`client.rs:323-343`) — единственная точка простановки fee_limit/expiration.
- `ConstantResult` (`client.rs:67-72`): парсится только `result`, `energy_used`
  (top-level), `success` (`parse_constant_result`, :546-557). `energy_penalty`
  **отбрасывается** (grep: строки `penalty` нет во всём репо).
- `TxInfo` (`client.rs:58-65`): `energy_used` ← `receipt["energy_usage_total"]`
  (`parse_tx_info`:538); `energy_penalty_total`/`energy_usage`/`origin_energy_usage`/
  `net_usage`/`net_fee` не читаются.
- Эндпоинты провайдера: getnowblock, `/jsonrpc` eth_chainId, getaccount,
  gettransactioninfobyid, triggerconstantcontract, broadcasthex. `getchainparameters`,
  `getcontractinfo`, `estimateenergy`, `getcontract` НЕ используются.
- API-ключ: header `TRON-PRO-API-KEY` цепляется в `crates/cast/src/tron.rs:23,44-47`
  (env `TRON_PRO_API_KEY`), провайдер сам env не читает.
- `TronConfig` (`crates/config/src/tron.rs:24-45`): `fee_limit` (1e9), `origin_energy_limit`
  (10_000_000), `user_fee_percentage` (100), `expiration` (60 c). CLI: только
  `--tron.fee-limit`/`--tron.expiration` (`crates/cli/src/opts/tron.rs:15-22`).
- chain_id в локальных tron-тестах: `local_evm_env` → `cfg_env(self.env.chain_id
  .unwrap_or(DEV_CHAIN_ID))` (`crates/evm/core/src/opts.rs:278`), `DEV_CHAIN_ID = 31337`
  (`crates/common/src/constants.rs:9`). `is_tron()` и chain_id никак не связаны ⇒
  `block.chainid == 31337` в non-fork tron-тесте (ломает EIP-712/permit domain).
- gas-report: `with_tron` (`crates/forge/src/gas_report.rs:70-80`), сборка
  `crates/forge/src/cmd/test/mod.rs:2803-2813`. **Deployment Energy = 0 для depth>1
  create**: `contract_info.gas = trace.gas_used` (:158) стоит ПОСЛЕ depth-guard'а (:150
  `if trace.depth > 1 ... return`), а `size`/`deployment_bandwidth` (:137-145) — ДО него.
- `cast estimate` (`crates/cast/src/cmd/estimate.rs:83-90`): ветки только tempo/ethereum,
  tron-пути НЕТ (grep-confirmed).

### F-6. Трейсы и base58 (скаут 2026-07-23)

- Рендер дерева трейсов — внешний крейт `revm-inspectors = 0.41.1` (`Cargo.toml:416`, без
  patch/fork): `TraceWriter` из `crates/evm/traces/src/lib.rs:235-240`. Хука форматтера
  адресов нет. Внутрирепные швы: (a) label-карта `CallTraceDecoder.labels`
  (`decoder/mod.rs:167`, рендер :622-623 — label подставляется вместо hex);
  (b) стрингификация декодированных аргументов `decode_function_input`
  (`decoder/mod.rs:731-762`) — `DynSolValue::Address` → hex Display.
- Сборка декодера в forge test: `crates/forge/src/cmd/test/mod.rs:2781-2801`;
  `config.networks.is_tron()` уже в скоупе строкой :2812. Прочие сборки декодера:
  `script/src/execute.rs`, `cast/src/debug.rs`, `chisel/src/dispatcher.rs`,
  `anvil/src/config.rs`.
- base58-хелперы: `foundry_tron_primitives::address::{to_base58 (:30), to_hex41 (:39),
  parse (:80)}`; уже используются в broadcast-артефактах (`script/src/tron.rs:225,362`).

### F-7. tron-solc и solc-bin (скаут 2026-07-23, live)

- `resolve_tron_solc(version, offline)` (`crates/tron/solc/src/lib.rs:220`, ядро
  `resolve_in` :226-264): платформа-гейт → **пин-гейт** (`NoPin` — жёсткий, без пина не
  резолвится даже из кэша) → кэш+sha256 → offline-гейт → download (GitHub release
  `tv_{version}`, ассеты solc-macos/solc-static-linux/solc-windows.exe, 3 ретрая) →
  sha256 → атомарная запись. Ошибки: `UnsupportedPlatform/NoPin/Offline/CorruptedCache/
  ChecksumMismatch/NoHome/Http/Io` (:36-89).
- `pins.rs`: `PINS` (0.8.23–0.8.27 × 3 платформы, :60-139) + `LONG_VERSIONS` (:51-57).
  `default_version() = 0.8.27` (`lib.rs:156-158`).
- Шов конфига: `ensure_solc` (`crates/config/src/lib.rs:1460-1471`) — tron + не-Local →
  резолвер; `SolcReq::Local` — явный оверрайд мимо пинов.
- solc-bin live: рабочие URL `https://tronprotocol.github.io/solc-bin/{macosx-amd64|
  linux-amd64|windows-amd64}/list.json`; схема `{"builds":[{path, version, build,
  longVersion, keccak256, sha256(0x-префикс), urls:[]}]}`; новейшая версия **0.8.28**
  (`0.8.28+commit.9c4253d2`) на всех трёх платформах; ARM-листов нет (404).
  Бинарники хостятся как `https://tronprotocol.github.io/solc-bin/{list_key}/{path}`.
- sha256 0.8.28 (сняты raw curl'ом 2026-07-23, авторитетны):
  - macosx-amd64: `e492e14fd3da07e65830c1189758ac34f8a3f06c3f59df42f5c5f6a744e0dbd2`
  - linux-amd64: `0eba121b08e9fbc1019e71bb6d36467a7f653929af9f51a793a17688d859f856`
  - windows-amd64: `000b24310f0b849d886b280908fde4c2dd63658bca3ffb4909ac7d9c6ab647e9`

### F-8. Cheatcode-швы (скаут 2026-07-23)

`crates/cheatcodes/src/evm.rs`: `chainId` :495-502, `coinbase` :504-510 (`set_beneficiary`
— env-driven, работает), `fee` :525-542, `prevrandao` :544-568, `txGasPrice` :638-657.
Tron-опкоды-оверрайды (`evm/tron/mod.rs:173-221`) env НЕ читают: `op_basefee` — константа,
`op_gasprice`/`op_difficulty` — жёсткий 0 ⇒ `vm.fee`/`vm.txGasPrice`/`vm.prevrandao` на
tron — тихие no-op. Прецедент network→env: `bypass_prevrandao`
(`crates/evm/networks/src/lib.rs:279`) → `crates/evm/core/src/utils.rs:92`.

---

## Задача I1 — precompile-кламп: таблица = ровно java-tron

**Файлы:** `crates/evm/core/src/evm/tron/mod.rs` (шов :137-140),
`crates/evm/core/src/evm/tron/precompiles.rs` (докстрока :24-29, тесты),
`crates/evm/networks/src/tron.rs` (labels — сверить, не менять без нужды).

**Суть:** заменить «extend поверх спек-базы revm» на полную подмену: карта precompile'ов
tron-EVM строится ТОЛЬКО из `tron_precompiles()`. Эфирная база (в т.ч. Osaka: BLS12-381
0x0b–0x11, P256Verify 0x100) не должна протекать ни при каком `evm_version` конфига.

- [ ] **Шаг 1 (тест, красный).** В `precompiles.rs::tests` (или `mod.rs::tests`) добавить
  контраст-тесты через `TronEvmFactory.create_evm` + `transact_raw` с CALL на адреса
  `0x0b` (BLS G1ADD) и `0x100` (P256Verify) с валидным для Ethereum входом:
  на tron-фабрике вызов должен вести себя как вызов пустого аккаунта — success,
  `output.is_empty()`, потрачена только энергия CALL (40) + окружение; на
  `EthEvmFactory` с Osaka-спеком — отличаться (непустой выход или иная энергия).
  Дополнительно: property-тест «адресный список карты == адресный список
  `tron_precompiles()` + `STUBS`» — ни одного лишнего адреса.
- [ ] **Шаг 2.** Механика подмены в `inject_tron_extensions`: вместо
  `inner.precompiles.extend_precompiles(...)` — сконструировать пустую/чистую
  `PrecompilesMap` и наполнить её только tron-набором. Конкретные API-варианты по
  предпочтению: (а) конструктор пустой карты в `alloy_evm::precompiles`
  (`PrecompilesMap::new(..)`/`From<Precompiles>` от пустого `Precompiles`); (б) если
  пустого конструктора нет — взять минимальную спек-базу (`Precompiles::new(
  PrecompileSpecId::HOMESTEAD)`; это 0x01–0x04, все четыре адреса наши же оверрайды
  перекрывают) и extend'ить tron-набором — леак невозможен, т.к. HOMESTEAD ⊂ tron-набор.
  Вариант (б) допустим только если (а) реально отсутствует в alloy_evm 0.x текущего
  workspace — зафиксировать выбор в комментарии с причиной.
- [ ] **Шаг 3.** Обновить докстроку `precompiles.rs:24-29`: «P256 intentionally absent»
  теперь должно быть правдой по построению; упомянуть кламп и почему вызов
  неизвестного адреса = пустой аккаунт (java-tron).
- [ ] **Шаг 4.** Foundry-путь: проверить, что `get_networks().inject_precompiles`
  (`mod.rs:298`) и `TRON_PRECOMPILES`-labels не возвращают эфирные адреса (скаут:
  `inject_precompiles` для tron — no-op, labels — 33 наших адреса; только сверить тестом
  «каждый label-адрес есть в карте»).
- [ ] **Шаг 5.** `cargo test -p foundry-evm-core tron` зелёный; sandbox `forge test` 5/5.
- [ ] **Шаг 6.** Коммит `fix(tron): clamp precompile map to the java-tron set`.

**Гейт:** контраст-тесты шага 1 зелёные; grep-подтверждение, что `extend_precompiles`
на спек-базе больше не вызывается на tron-пути.

**Osaka-задел (НЕ реализация):** при активации `allowTvmOsaka` на Nile/mainnet таблицу
придётся версионировать (TIP-7883 ModExp, P256Verify, strict-ABI 0x09/0x0a). В этом этапе —
только предупреждение из I4 (fetch `getAllowTvmOsaka` → warn при 1) + TODO-якорь в докстроке.

---

## Задача I2 — transfer-баг: value-only вызов контракта

**Файлы:** `crates/tron/provider/src/client.rs` (новый метод), `crates/cast/src/cmd/send.rs`
(:213-223), `crates/script/src/tron.rs` (:258-287 + broadcast-петля),
тесты: `crates/tron/provider` (fixtures), `crates/forge/tests/cli/tron.rs`.

**Суть:** пустая calldata ≠ native transfer. Если целевой адрес — контракт, строить
`TriggerSmartContract` с пустым `data` (легальный вызов payable fallback/receive);
`TransferContract` — только для не-контрактов.

- [ ] **Шаг 1 (провайдер, тест-первым).** Оффлайн-fixture-тест на новый метод:
  `pub async fn is_contract(&self, address: Address) -> Result<bool, TronError>` —
  POST `/wallet/getcontract` `{"value": <hex41>, "visible": false}`; контракт ⇒ в ответе
  есть `bytecode`/`contract_address`; обычный аккаунт ⇒ `{}` ⇒ false. Фикстуры: реальный
  ответ getcontract для USDT (mainnet) и для EOA (снять curl'ом, положить рядом с
  существующими fixtures крейта). Транзиентные ошибки — пробрасывать (решение о фолбэке
  принимает вызывающий).
- [ ] **Шаг 2 (cast).** В `run_tron` (`send.rs:218`) ветка `data.is_empty()`:
  `let raw = if data.is_empty() && !provider.is_contract(dest).await? {
  build_transfer_raw(..) } else { build_trigger_raw(from, dest, value_sun, data, ..) }`.
  `build_trigger_raw` с пустым `data` — проверить, что билдер не отвергает пустой вектор
  (`client.rs:366`); если отвергает — снять запрет с тестом.
- [ ] **Шаг 3 (script).** В broadcast-петле `script/src/tron.rs`: перед классификацией
  собрать множество `to`-адресов кандидатов-Transfer (пустой input), одним проходом
  спросить `is_contract` (кэш `HashMap<Address, bool>` на весь broadcast), и в `classify`
  (или сразу после неё) апгрейдить `TronCall::Transfer { to, value }` →
  `TronCall::Trigger { to, data: vec![], value }` для контрактов. `classify` остаётся
  синхронной — карту передавать параметром.
- [ ] **Шаг 4 (юнит).** Тест классификации: (to=контракт, data=∅) → Trigger с пустым data;
  (to=EOA, data=∅) → Transfer; (to=контракт, data≠∅) → Trigger. Плюс cast-путь под
  cfg(test) или через существующий паттерн CLI-тестов tron (`crates/forge/tests/cli/tron.rs`,
  `crates/cast/tests/cli/`) с mock-ответами, как сделано в оффлайн-тестах провайдера.
- [ ] **Шаг 5 (live, гейт `TRON_LIVE=1`).** Nile: задеплоить минимальный
  `contract PayableSink { receive() external payable {} }` (добавить в sandbox или
  testdata), `cast send <sink> --value 1000000` без сигнатуры → нода принимает, receipt
  SUCCESS, баланс контракта вырос. Это прямое воспроизведение бага до фикса.
- [ ] **Шаг 6.** Коммит `fix(tron): route value-only contract calls via TriggerSmartContract`
  (+ тело: нода отвергает TransferContract на контракт-адрес).

**Гейт:** live-тест шага 5 зелёный на Nile.

---

## Задача I3 — tron-solc: пин 0.8.28 + динамический резолв из solc-bin

**Файлы:** `crates/tron/solc/src/pins.rs`, `crates/tron/solc/src/lib.rs`,
`crates/config/src/lib.rs` (шов не меняется), тесты крейта.

- [ ] **Шаг 1 (пины).** Добавить в `PINS` три строки 0.8.28 (sha256 из F-7, дословно) и в
  `LONG_VERSIONS` — `0.8.28+commit.9c4253d2`. Оффлайн-тест: `pinned_sha256("0.8.28", ..)`
  возвращает ожидаемые значения для всех трёх платформ.
- [ ] **Шаг 2 (динамический резолв, тест-первым).** Новое поведение `resolve_in` при
  `NoPin` и `!offline`: перед ошибкой — попытка resolv'а через solc-bin:
  1. GET `https://tronprotocol.github.io/solc-bin/{list_key}/list.json` (тот же
     `download_bytes` с ретраями);
  2. найти в `builds[]` запись с `version == {ver}`; нет ⇒ прежняя ошибка `NoPin`
     (сообщение дополнить: «not in tronprotocol solc-bin list either»);
  3. sha256 из поля `sha256` (срезать `0x`, lowercase), URL бинарника =
     `https://tronprotocol.github.io/solc-bin/{list_key}/{entry.path}`;
  4. дальше общий путь: download → sha256-verify → атомарная запись в тот же кэш
     `~/.foundry-tron/solc/tron-solc-{ver}`; `long_version` брать из `entry.longVersion`
     (нужно для `tronscan_compiler_string` — расширить lookup: пины → потом кэш
     list-ответа; для верификации TRONSCAN версий вне пинов).
  Порядок доверия: пины (compile-time) остаются приоритетом; solc-bin — тот же источник,
  из которого сняты сами пины (задокументировать в докстроке). `offline` ⇒ поведение не
  меняется (никакой сети, `NoPin` как раньше).
  Тесты: оффлайн — парсинг list.json из fixture (положить реальный сокращённый
  list.json в testdata крейта), выбор записи, построение URL, отказ при отсутствии
  версии; download-гейт `TRON_SOLC_DOWNLOAD=1` — резолв версии, которой нет в пинах
  (например 0.8.24 временно исключить из «видимости» нельзя — вместо этого гейт-тест
  резолвит 0.8.28 ЧЕРЕЗ list-путь, форсируя ветку параметром/тест-хуком `resolve_in`
  без пина: внутренняя `fn resolve_via_list(dir, version, platform)` тестируется напрямую).
- [ ] **Шаг 3 (дефолт).** `default_version()` поднять до 0.8.28 ТОЛЬКО после зелёного
  гейта: sandbox `tron-counter` c `solc = "0.8.28"` собирается и `forge test` 5/5
  (download-гейт). Если сборка сломана — оставить 0.8.27 дефолтом, 0.8.28 доступен
  явным указанием; решение зафиксировать в STATUS.
- [ ] **Шаг 4.** `cargo test -p foundry-tron-solc` (оффлайн) + `TRON_SOLC_DOWNLOAD=1`
  гейт; `cargo test -p foundry-config tron` — не сломан.
- [ ] **Шаг 5.** Коммит(ы): `feat(tron): pin tron-solc 0.8.28`,
  `feat(tron): resolve unpinned tron-solc versions from solc-bin`.

**Гейт:** `solc = "0.8.28"` в foundry.toml подхватывается без правки кода тулчейна;
неизвестная версия при `offline = true` по-прежнему детерминированно падает `NoPin`.

**Ответ на вопрос пользователя (зафиксировать в USER_GUIDE, задача I8/доки):** менять
версию tron-solc можно из конфига (`solc = "X.Y.Z"`); после этой задачи — включая версии,
вышедшие после релиза foundry-tron (тянутся из официального solc-bin с чексуммой);
`solc = "/путь"` как and-остаётся явным оверрайдом.

---

## Задача I4 — идентичность сети: chainid-дефолт, BASEFEE из env, предупреждения no-op читкодов

**Файлы:** `crates/evm/networks/src/lib.rs` + `crates/evm/networks/src/tron.rs` (константы),
`crates/evm/core/src/opts.rs` (:275-288), `crates/evm/core/src/evm/tron/mod.rs`
(op_basefee), `crates/cheatcodes/src/evm.rs` (:525-568, :638-657),
`crates/forge/tests/cli/tron.rs`, sandbox.

- [ ] **Шаг 1 (константы).** В `foundry-evm-networks::tron`: `pub const
  TRON_MAINNET_CHAIN_ID: u64 = 728126428;` и `pub const TRON_NILE_CHAIN_ID: u64 =
  3448148188;` (уже известны проекту: broadcast-артефакты плана D лежат под 3448148188).
- [ ] **Шаг 2 (тест, красный).** Forge-CLI-тест (`forgetest!` в
  `crates/forge/tests/cli/tron.rs`): проект `network = "tron"` без `chain_id`,
  тест-контракт ассертит `block.chainid == 728126428`. Сейчас упадёт (31337).
- [ ] **Шаг 3.** `local_evm_env` (`opts.rs:278`): дефолт chain_id сетевой —
  `self.env.chain_id.unwrap_or(if self.networks.is_tron() { TRON_MAINNET_CHAIN_ID }
  else { DEV_CHAIN_ID })`. Явный `chain_id` в конфиге (в т.ч. Nile) побеждает как раньше.
  Fork-путь не трогать (:220 — chain id берётся у ноды). Сверить sandbox
  `testTronChainId` (он мог полагаться на 31337/явный конфиг — привести к новому дефолту).
- [ ] **Шаг 4 (BASEFEE через env).** Сделать `vm.fee` работающим БЕЗ потери верности:
  `op_basefee` читает `block_env.basefee` (как ванильный revm — тогда кастомный опкод
  можно просто НЕ вставлять, оставив статик-энергию 2 через `insert_gas`), а верный
  дефолт 100 обеспечивает env-setup: в `local_evm_env` для tron `block.basefee =
  TRON_ENERGY_FEE_SUN as u64`; в fork-пути (`fork_evm_env`/tron-ветка) — принудительно
  выставлять 100 (или значение из chain-params I5, когда доступно), т.к. `/jsonrpc`
  блок отдаёт эфирный baseFee, не energy price. Тест из energy.rs
  `basefee_returns_tron_energy_fee` остаётся зелёным (дефолт), плюс новый тест:
  `vm.fee(7)` в tron-тесте ⇒ BASEFEE возвращает 7.
- [ ] **Шаг 5 (честные предупреждения).** `vm.prevrandao`/`vm.difficulty` и
  `vm.txGasPrice` на tron остаются несемантичными (опкоды жёстко 0 — верно для Tron):
  в `apply_stateful` соответствующих читкодов (`evm.rs:544-568`, :638-657) при
  `is_tron` (доступ к network-флагам — через тот же канал, каким `bypass_prevrandao`
  доходит до `utils.rs:92`; если в `CheatsCtxt` флага нет — пробросить его в
  `CheatsConfig` при создании инспектора) — один раз за прогон эмитить warning в стиле
  существующих deprecation-warnings cheatcodes («no effect on tron: DIFFICULTY/GASPRICE
  are hardwired to 0 by the TVM»). НЕ ошибка — тесты общих либ не должны падать.
- [ ] **Шаг 6.** `vm.coinbase` — уже работает (env-driven, F-8). В доки (USER_GUIDE):
  локальный дефолт COINBASE = 0x0, на живом Tron — 0x41-адрес SR; не переопределяем.
- [ ] **Шаг 7.** Коммиты: `feat(tron): default local chain id to tron mainnet`,
  `feat(tron): env-driven BASEFEE with faithful default`,
  `feat(tron): warn on no-op cheatcodes`.

**Гейт:** тест шага 2 зелёный; EIP-712-фикстура (permit-домен, sig-verify через
ecrecover-precompile) проходит на дефолтном chainid без ручного конфига.

---

## Задача I5 — chain-параметры: getchainparameters + различение сетей

**Файлы:** `crates/tron/provider/src/client.rs`, `crates/evm/core/src/evm/tron/energy.rs`
(константы → дефолты), `crates/config/src/tron.rs` (опц. оверрайды), потребители I6/I7.

- [ ] **Шаг 1 (тип+метод, тест-первым, оффлайн-fixture).**
  ```rust
  pub struct TronChainParams {
      pub energy_fee_sun: u64,          // getEnergyFee
      pub max_fee_limit_sun: u64,       // getMaxFeeLimit
      pub transaction_fee_sun: u64,     // getTransactionFee (bandwidth, sun/байт)
      pub memo_fee_sun: u64,            // getMemoFee
      pub dynamic_threshold: u64,       // getDynamicEnergyThreshold
      pub dynamic_increase_factor: u32, // getDynamicEnergyIncreaseFactor
      pub dynamic_max_factor: u32,      // getDynamicEnergyMaxFactor
      pub allow_tvm_osaka: bool,        // getAllowTvmOsaka (0/1)
  }
  ```
  + `impl Default` = live-значения 2026-07 (100 / 15e9 / 1000 / 1e6 / 5e9 / 2000 / 34000 /
  false) с докстрокой «оффлайн-дефолты, проба 2026-07-23». Метод
  `pub async fn get_chain_parameters(&self) -> Result<TronChainParams, TronError>` —
  POST `/wallet/getchainparameters`, маппинг по `key`, отсутствующий ключ ⇒ дефолт поля.
  Fixture — реальный (сокращённый до нужных ключей) ответ mainnet.
- [ ] **Шаг 2 (потребители-минимум).** (а) fork-режим: после установления tron-fork
  (`fork_evm_env`-ветка) — best-effort fetch (не валить прогон при сетевой ошибке):
  warn при `energy_fee_sun != TRON_ENERGY_FEE_SUN` («local energy price constant is
  stale») и при `allow_tvm_osaka` («node runs Osaka TVM; local model is pre-Osaka»).
  Wallet-база из fork-URL: замена суффикса `/jsonrpc` → `/wallet`; если суффикса нет —
  проп. (б) `TRON_ENERGY_FEE_SUN` остаётся compile-time дефолтом, но все НОВЫЕ
  потребители (I6-формулы) берут цену из `TronChainParams`.
- [ ] **Шаг 3 (live-гейт `TRON_LIVE=1`).** mainnet: fetch возвращает ровно дефолты
  (пока governance не сдвинул) — тест сравнивает и ПЕЧАТАЕТ диф при расхождении
  (это наш «датчик устаревания» в tron-live CI, план H6).
- [ ] **Шаг 4.** Коммит `feat(tron): fetch chain parameters from the node`.

**Гейт:** оффлайн-тесты + live-гейт; tron-live CI-workflow дополнен этим тестом.

---

## Задача I6 — петля оценки: energy_penalty, estimateenergy, `cast estimate`, fee_limit-формула

**Файлы:** `crates/tron/provider/src/client.rs`, `crates/cast/src/cmd/estimate.rs`,
`crates/cast/src/tron.rs`, `crates/cast/src/cmd/send.rs` + `crates/script/src/tron.rs`
(валидация fee_limit), `crates/config/src/tron.rs`.

- [ ] **Шаг 1 (провайдер, тест-первым, fixtures).**
  - `ConstantResult` + `pub energy_penalty: u64` (`v["energy_penalty"]`, default 0) —
    `parse_constant_result`; фикстура с реальным USDT-ответом (mainnet
    triggerconstantcontract `transfer(address,uint256)`: energy_used включает penalty).
  - `TxInfo` + `pub energy_penalty_total: u64`, `pub energy_usage_caller: u64`
    (`receipt.energy_usage`), `pub origin_energy_usage: u64`, `pub net_usage: u64`,
    `pub net_fee_sun: u64` (все default 0) — `parse_tx_info`. Существующие поля/тесты
    не ломать.
  - `pub async fn estimate_energy(&self, owner, contract, data) -> Result<Option<u64>,
    TronError>` — POST `/wallet/estimateenergy`; ответ с `energy_required` ⇒ Some;
    ответ-ошибка «this node does not support estimate energy» (и `result.code =
    NOT_SUPPORT...`) ⇒ Ok(None) — сигнал фолбэка; прочее ⇒ Err. Fixture обоих ответов.
  - `pub async fn get_contract_energy_factor(&self, contract) -> Result<u32, TronError>`
    — POST `/wallet/getcontractinfo`, `contract_state.energy_factor` (нет ⇒ 0). Fixture:
    USDT (34000) и свежий контракт (0/absent).
- [ ] **Шаг 2 (формула, чистая функция + юнит).** В `crates/cast/src/tron.rs` (или
  провайдере): `pub fn suggest_fee_limit_sun(energy_total: u64, params:
  &TronChainParams, buffer_pct: u64) -> u64` = `min(energy_total × energy_fee_sun ×
  (100+buffer_pct)/100, max_fee_limit_sun)`; buffer по умолчанию 20. Юнит-вектора,
  включая кламп на 15 000 TRX.
- [ ] **Шаг 3 (cast estimate).** Tron-ветка в `estimate.rs::run` (зеркально паттерну
  `call.rs:258-260 → run_tron`): построить calldata как в `cast call`; порядок:
  `estimate_energy` → если None, `trigger_constant`; получить
  `params = get_chain_parameters()` (сетевая ошибка ⇒ `TronChainParams::default()` +
  warn) и `bandwidth = estimate_call_bandwidth(..)`. Вывод (stdout — результат,
  как в CLI-контракте): text — таблица `energy_used / energy_penalty / bandwidth_bytes /
  suggested_fee_limit_sun / est_cost_trx` (cost = energy×price + bandwidth×
  transaction_fee); `--json` — те же поля объектом. CLI-снапшот-тесты в
  `crates/cast/tests/cli/` (mock/fixture-паттерн как у существующих tron-тестов; live —
  отдельным гейтом).
- [ ] **Шаг 4 (валидация fee_limit).** В `wrap_raw`-вызывающих путях (cast `run_tron`,
  script broadcast, forge create): перед broadcast — `fee_limit > max_fee_limit_sun` ⇒
  bail с формулой и подсказкой `--tron.fee-limit`; `fee_limit <= 0` ⇒ bail. Дефолт max —
  из `TronChainParams::default()` (fetch по best-effort, I5). Юнит + CLI-тест на ошибку.
- [ ] **Шаг 5 (receipt-вывод).** `cast send`/`forge create`/`forge script` на tron после
  подтверждения печатают (stderr, статус): `energy_usage_total (penalty P) | caller/origin
  split | net_usage | fee_trx` из расширенного TxInfo — сверка «оценка ↔ receipt» глазами.
- [ ] **Шаг 6 (live-гейт `TRON_LIVE=1`, mainnet read-only).** USDT:
  `cast estimate <USDT> "transfer(address,uint256)" <held-addr> 1 --from <held-addr>` —
  `energy_penalty > 0`, `energy_used ≈ base×4.4` (интервал, не точное число — фактор
  живой), suggested_fee_limit ≤ 15 000 TRX. Ноль TRX не тратится.
- [ ] **Шаг 7.** Коммиты: `feat(tron): parse energy penalty and receipt resource fields`,
  `feat(tron): cast estimate for tron with fee-limit suggestion`,
  `fix(tron): validate fee_limit against getMaxFeeLimit`.

**Гейт:** live USDT-оценка (шаг 6) зелёная; оценка и receipt-поля дают сходящуюся
арифметику `base = energy_used − energy_penalty`.

---

## Задача I7 — TIP-491 в симуляции: penalty в fork-режиме и gas-report; фикс Deployment Energy

**Файлы:** `crates/forge/src/gas_report.rs`, `crates/forge/src/cmd/test/mod.rs`
(:2803-2813), `crates/evm/core/src/fork/multi.rs`/`opts.rs` (доступ к wallet-базе),
`crates/config/src/tron.rs` (knob), testdata-фикстуры.

**Дизайн (зафиксировано).** Penalty НЕ вносится в интерпретатор (в revm нет per-address
множителя газа без переписывания метеринга; изменение `gasleft()` внутри горячих
контрактов — документированное ограничение). Penalty считается ПОСТ-ФАКТУМ из trace-арены:
собственная энергия узла = `node.gas_used − Σ(children.gas_used)`; penalty узла =
`own × factor(node.address)/10000`; penalty транзакции = сумма по узлам. Факторы: fork —
`get_contract_energy_factor` по уникальным адресам арены (кэш на прогон; wallet-база —
из fork-URL, шов I5); non-fork — все факторы 0 (свежие контракты, верно по определению).

- [ ] **Шаг 1 (фикс Deployment Energy=0, тест-первым).** CLI-фикстура: фабрика
  (`new Child()` внутри контракта) + `--gas-report` — сейчас Child: Energy 0 при
  непустых Size/Bandwidth. Фикс: перенести запись `contract_info.gas = trace.gas_used`
  (gas_report.rs:158) ДО depth-guard'а (:150), рядом с size/bandwidth (:137-145),
  сохранив условие is_create; НЕ ломая существующую семантику EVM-репортов
  (guard оставляет прежнее поведение для frames/вызовов). EVM-снапшоты cmd.rs:2039+
  байт-в-байт.
- [ ] **Шаг 2 (penalty-расчёт, чистая функция + юнит).** В gas_report.rs (или рядом):
  `fn dynamic_penalty(arena: &CallTraceArena, factors: &HashMap<Address, u32>) -> u64`
  по формуле выше. Юнит на руками собранной арене: родитель 100k (свой 40k) с factor
  34000 + ребёнок 60k factor 0 ⇒ penalty 40k×3.4 = 136k.
- [ ] **Шаг 3 (сбор факторов, fork-only).** Прокинуть в `GasReport`/`with_tron`
  опциональный источник факторов: `tron.dynamic_energy = true` (новое поле TronConfig,
  default true) + fork-URL из `evm_opts`; при non-fork или выключенном knob — пустая
  карта. Запрос — блокирующе-редкий (уникальные адреса, `RuntimeOrHandle`-паттерн как в
  tron-solc), ошибки сети ⇒ warn + фактор 0 (репорт остаётся base-only, но честно
  предупреждает).
- [ ] **Шаг 4 (рендер).** Tron-репорт: колонка «Energy (min/avg/…)» остаётся base;
  добавить `+ Penalty` в deployment- и function-строки только при ненулевой карте
  факторов: text — доп. колонка `Penalty (avg)`, JSON — `"energy_penalty"` опциональным
  полем по паттерну bandwidth (`skip_serializing_if`). EVM-выводы неизменны.
- [ ] **Шаг 5 (live-гейт `TRON_LIVE=1`, mainnet fork read-only).** Форк-тест по образцу
  плана G (USDT ABI/storage уже в `crates/forge/tests/cli/tron.rs`): вызвать
  `USDT.transfer` в форке, посчитать `base_local + dynamic_penalty` и сверить с
  `triggerconstantcontract.energy_used` того же вызова на той же ноде: расхождение ⇽
  только живым дрейфом фактора между fetch'ами — допуск: точное совпадение при равном
  факторе в обеих точках (перечитать фактор после сверки, при изменении — перезапуск
  сверки, максимум 2 попытки).
- [ ] **Шаг 6.** Доки: USER_GUIDE — раздел «Dynamic energy (TIP-491)»: что включено где,
  ограничение gasleft(), как читать Penalty-колонку, ссылка на formулу fee_limit.
- [ ] **Шаг 7.** Коммиты: `fix(tron): record deployment energy for nested creates`,
  `feat(tron): TIP-491 dynamic energy penalty in fork simulations and gas report`.

**Гейт:** live-сверка шага 5 (exact при стабильном факторе); non-fork прогоны
байт-в-байт неизменны при пустой карте факторов.

---

## Задача I8 — base58-адреса в трейсах и выводах

**Файлы:** `crates/evm/traces/src/decoder/mod.rs` (:167, :622-623, :731-762),
`crates/forge/src/cmd/test/mod.rs` (:2781-2801), `crates/script/src/execute.rs`,
`crates/cast/src/debug.rs`, `crates/chisel/src/dispatcher.rs`, доки.

**Дизайн (зафиксировано).** `TraceWriter` — внешний крейт без хука форматтера (F-6);
форкать revm-inspectors не будем. Два внутрирепных шва покрывают запрос «base58 везде»:

1. **Идентификация узлов:** незалейбленный адрес получает label = его base58-форма ⇒
   TraceWriter рендерит `TEsg…::setNumber(...)` вместо `0x…::setNumber`. Явные ярлыки
   (`vm.label`, известные контракты) остаются приоритетными.
2. **Декодированные аргументы:** address-значения в args/returns/logs рендерятся base58.

- [ ] **Шаг 1 (декодер).** `CallTraceDecoder`: поле `tron_addresses: bool` + builder
  `CallTraceDecoderBuilder::with_tron_addresses(bool)`. (а) В точке label-фолбэка
  (`decoder/mod.rs:622-623`): при `tron_addresses` и отсутствии label — вернуть
  `Some(foundry_tron_primitives::to_base58(trace.address))`. (б) В стрингификации
  аргументов/ретёрнов/логов (`decode_function_input` :731-762 и парные decode-пути):
  address-значения (`DynSolValue::Address`, включая вложенные массивы/структуры —
  общий helper обхода) → base58. Внимание на зависимость: если `foundry-evm-traces` не
  зависит от `foundry-tron-primitives` — добавить лёгкую dep (primitives без heavy deps)
  либо принять форматтер-замыкание `Option<fn(Address) -> String>` — предпочесть
  замыкание, чтобы не тащить dep в общий крейт (решение исполнителя, зафиксировать).
- [ ] **Шаг 2 (включение).** Все точки сборки декодера, где известен конфиг:
  forge test (`test/mod.rs:2781-2801`, рядом `is_tron` :2812), script
  (`execute.rs`), cast debug, chisel — `builder.with_tron_addresses(
  config.networks.is_tron())`. anvil — пропустить (anvil-tron вне скоупа этапа).
- [ ] **Шаг 3 (тесты).** Юнит декодера: арена с вызовом и address-аргументом при
  `tron_addresses=true` ⇒ рендер содержит `T…`-строки и НЕ содержит `0x…` для этих
  адресов; при false — прежний hex (EVM-снапшоты не тронуты). CLI-снапшот: tron-проект
  `forge test -vvvv` — трейс с base58 (обновить/добавить фикстуру в
  `crates/forge/tests/cli/tron.rs`).
- [ ] **Шаг 4 (границы).** console.log-вывод (форматируется контрактной либой) и
  golden-EVM-снапшоты остаются hex — задокументировать в USER_GUIDE (`cast tron-address`
  как конвертер). `vm.label` пользователя всегда побеждает base58-фолбэк.
- [ ] **Шаг 5.** Коммит `feat(tron): render addresses as base58 in traces and decoded args`.

**Гейт:** `-vvvv` трейс tron-теста показывает адреса в base58; EVM-снапшоты байт-в-байт.

---

## Порядок, ветка, процесс

Порядок исполнения: **I1 → I2 → I3 → I4 → I5 → I6 → I7 → I8** (I5 — фундамент для I6/I7;
I1/I2/I3 независимы и могут идти параллельными worktree при желании).

- Ветка `tron-stage4` от `master`; PR → `master` (мейнлайн, см. STATUS).
- Конвенции: sh_-макросы (никаких println), doc-комменты перед атрибутами, тесты с
  `fork` в имени для fork-тестов, live-гейты `TRON_LIVE=1` / `TRON_SOLC_DOWNLOAD=1`.
- Каждая задача: тест-первым, отдельные коммиты, Opus-верификация с перепроверкой
  доменных фактов по первоисточникам (java-tron @develop, live mainnet/Nile), Fable-ревью
  после каждых двух задач.
- По завершении: обновить `docs/tron/STATUS.md` (таблица этапов + snapshot-константы),
  USER_GUIDE (dynamic energy, cast estimate, base58, solc-версии), tron-live CI (I5-гейт).

## Вне скоупа этапа (DEFER, зафиксировать в STATUS)

- Osaka-вариант precompile/энергомодели (ждём активации на Nile; сторожок — warn из I5).
- anvil-tron / chisel-tron как настоящие TVM-окружения (бинарники остаются vanilla-EVM).
- In-loop метеринг penalty (gasleft() внутри hot-контрактов), TRC-10 семантика (0xD0–0xD3
  остаются стабами), stake/gov-опкоды 0xd5–0xdf, multisig/permission_id/memo в билдере,
  глубина вызовов 64 (revm CALL_STACK_LIMIT), учёт 1.1 TRX activation-burn в оценках.
- `vm.tronSetEnergyFactor(address,uint32)` читкод для локального моделирования penalty.
