# foundry-tron: форк Foundry для Tron (TVM) — дизайн

Дата: 2026-07-10
Статус: утверждён (обсуждение в сессии), ожидает имплементационного плана

## 1. Цель и мотивация

Портировать Foundry-опыт разработки на Tron: быстрые локальные тесты без докера,
компиляция, деплой и CLI-утилиты с API, максимально идентичным Foundry.
Существующий инструмент TronBox (форк Truffle) медленный, требует докер-ноду
java-tron (TRE) для любых тестов, не имеет фаззинга, cheatcodes, gas-репортов
и нативных Solidity-тестов.

Аудитория: сначала внутренняя миграция контрактов (Solidity 0.8.20+) на Tron,
архитектура сразу закладывается под последующий OSS-релиз (ниша в экосистеме
Tron пуста — форка Foundry под TVM не существует).

## 2. Ключевые факты исследования (основания решений)

1. **База — не ванильный Foundry.** Репозиторий `foundry/` в этом проекте —
   мультисетевой форк под сеть Tempo (tempoxyz), Foundry 1.7.2. В нём уже есть
   сети Ethereum / Optimism / Tempo через абстракции:
   - `FoundryEvmNetwork` / `FoundryEvmFactory` — `crates/evm/core/src/evm/mod.rs`
   - `NetworkVariant` / `NetworkConfigs` — `crates/evm/networks/src/lib.rs`
   - `FoundryTxEnvelope` (кастомные типы транзакций, прецедент Tempo ty=0x76) —
     `crates/primitives/src/transaction/envelope.rs`
   - `FoundryTransactionBuilder<N>` (кастомная сборка/подпись) —
     `crates/common/src/transactions/builder.rs`
   - диспетчер сети — `crates/forge/src/cmd/test/mod.rs` (`dispatch_network`)
   Эталон для копирования — `crates/evm/core/src/evm/tempo.rs`.

2. **TVM ≈ Cancun-EVM.** После java-tron v4.8.x (Kant/Democritus/Hypatia):
   PUSH0, TLOAD/TSTORE, MCOPY, EIP-6780 SELFDESTRUCT; BLOBHASH/BLOBBASEFEE —
   заглушки, возвращающие 0. Байткод общий с EVM → revm остаётся движком
   исполнения, новая VM не нужна.

3. **Отличия TVM, требующие патчей revm (этап 2):**
   - Tron-опкоды `0xD0–0xDF` (TRC-10: CALLTOKEN/TOKENBALANCE/…; ISCONTRACT;
     FREEZE/VOTE/DELEGATE-семейство);
   - precompiles: `0x09` = BatchValidateSign (TIP-43), `0x0a` = ValidateMultiSign
     (TIP-60), Ripemd160 → `0x20003`, Blake2F → `0x20009`, shielded-TRC20 —
     `0x1000001–0x1000004`;
   - CREATE2: `addr = keccak256(0x41 ‖ sender21 ‖ salt ‖ keccak256(initcode))[12..]`
     (префикс `0x41`, sender — 21-байтовый, БЕЗ `0xff`; **уточнено 2026-07-12** по
     `WalletUtil.generateContractAddress2`, подтверждено golden на Nile);
   - GASPRICE = 0 (CompatibleEvm выкл. на mainnet/Nile), BASEFEE = `getEnergyFee()`
     = **100** SUN (**уточнено 2026-07-12** live на mainnet и Nile — прежнее
     «energyPrice/420» неверно); CHAINID mainnet = 728126428;
   - модель ресурсов: energy = pre-EIP-150 (FRONTIER) gas-таблицы revm + TVM-дельты
     (**уточнено 2026-07-12**: «стоимости опкодов совпадают с EVM-gas» верно ТОЛЬКО
     для FRONTIER-базиса, НЕ для CANCUN — cold/warm EIP-2929 и EXP-byte расходятся;
     MLOAD/MSTORE/MSTORE8 = SPECIAL_TIER 1; refund'ов нет; НЕТ EIP-170/EIP-3860) +
     bandwidth (байты сериализованной транзакции), `fee_limit` в TRX.
   Перед реализацией нумерация precompiles и поведение опкодов сверяются с
   исходниками java-tron (доки противоречивы).

4. **Главная стоимость — транзакции и RPC, не VM:**
   - транзакции — protobuf (`raw_data` + `contract[]`), а не RLP;
     txID = sha256(raw_data); подпись secp256k1 по sha256;
   - вместо nonce — TAPOS (`ref_block_bytes/hash`) + `expiration` (~60 сек);
   - `eth_sendRawTransaction` в java-tron **отсутствует**; запись — только
     HTTP `/wallet/*` (`triggersmartcontract`, `deploycontract`,
     `broadcasttransaction`);
   - read: частичный `eth_*` JSON-RPC есть (`eth_call`, `eth_getBalance`,
     `eth_getLogs`, `eth_getBlockByNumber`, `eth_estimateGas`→energy), но
     state-запросы работают только по tip чейна;
   - адрес деплоя контракта выводится из txID, а не из sender+nonce.

5. **Компилятор — tron-solc** (форк tronprotocol): бинарники на
   `tronprotocol.github.io/solc-bin` (sha256 в list.json), версии до 0.8.26,
   отставание от апстрима — единицы миноров; standard-json и формат артефактов
   solc-совместимы (подтверждено кодом TronBox). Наши контракты 0.8.20+ покрыты.
   Foundry поддерживает drop-in: `solc = "/path"` в foundry.toml.

6. **Адреса:** Tron-адрес = 21 байт (`0x41` + 20 байт keccak(pubkey)[12:]),
   на I/O — base58check (`T…`). Ключи/деривация — те же secp256k1.

7. **Rust-референсы:** `andelf/opentron` (protobuf-типы протокола Tron),
   foundry-zksync (паттерн форка), TronBox `src/components/TronWrap` (карта
   Tron-параметров: feeLimit=1e9 SUN дефолт, originEnergyLimit=1e7,
   userFeePercentage=100, 1 TRX = 1e6 SUN).

## 3. Утверждённые решения

| Решение | Выбор |
|---|---|
| Скоуп MVP | forge build + forge test + forge script/деплой + cast (anvil/chisel — вне скоупа) |
| Семантика VM | Гибрид: revm с Tron-шимами по умолчанию + опциональные прогоны против java-tron (fork-режим / докер) |
| Стратегия форка | Прямой форк текущего репо (Tempo-форк Foundry) с точечными патчами + новые tron-крейты внутри workspace; периодический rebase |
| База форка | Tempo-форк (tempoxyz), не апстрим foundry-rs — ради готовой мультисетевой абстракции |
| Путь реализации | Поэтапно: этап 1 «пайплайн» → этап 2 «точность» → этап 3 «полировка/OSS» |

## 4. Архитектура

### 4.1 Структура

```
foundry-tron/                          (форк текущего репо)
├── crates/
│   ├── evm/core/src/evm/tron.rs       TronEvmNetwork: FoundryEvmNetwork
│   ├── evm/networks/                  NetworkVariant::Tron + precompiles-инъекция
│   ├── evm/hardforks/                 TronHardfork → SpecId::CANCUN
│   ├── tron/primitives/               НОВЫЙ: адресный кодек, protobuf-tx,
│   │                                  sha256-txID, подпись, SUN/TRX
│   ├── tron/provider/                 НОВЫЙ: клиент TronGrid/java-tron
│   └── tron/revm/                     НОВЫЙ (этап 2): инструкции 0xD0–0xDF,
│                                      precompiles, CREATE2-0x41, energy
```

Tron-крейты живут внутри workspace (не внешними git-репо, как tempo-*):
меньше репозиториев на этапе внутренней разработки; вынос — при OSS-релизе.

### 4.2 Сквозной принцип адресов

Внутри ядра адрес остаётся 20-байтовым `alloy_primitives::Address`.
Префикс `0x41` и base58check появляются только на границах I/O:
- парсинг CLI-аргументов: принимаются `T…` (base58check), `41…`-hex и `0x…`;
- вывод (консоль, трейсы, broadcast-артефакты): base58 при `network = "tron"`;
- protobuf-сериализация транзакций (21-байтовый формат).
Точки правок: парсер `NameOrAddress`-аргументов (`crates/cast/src/opts.rs`),
форматтер `UIfmt` (`crates/common/fmt/src/ui.rs`), console-форматтер
(`crates/common/fmt/src/console.rs`), деривация ключ→адрес
(`crates/common/src/wallet.rs` — добавление 0x41 только на выводе).

### 4.3 Компиляция (forge build)

- **Этап 1 (drop-in, 0 правок):** `solc = "<путь к tron-solc>"` +
  фиксированный `evm_version`. Артефакты/remappings/линковка — без изменений.
- **Этап 2 (резолвер):** скачивание бинарников с
  `tronprotocol.github.io/solc-bin` (sha256-проверка по list.json) в
  `~/.foundry-tron/solc/`, автовыбор версии по pragma. Правки:
  `crates/config/src/lib.rs` (`ensure_solc()`, `SolcReq`).
- **Проверки первой недели (риски):** поддержка via-ir в tron-solc;
  наличие нативных бинарников под macOS/arm (у TronBox — wasm soljson;
  если нативных нет — собираем tron-solc из исходников и хостим сами).

### 4.4 Тесты (forge test)

**Этап 1 — «наивный» revm.** `TronEvmNetwork` с минимальной конфигурацией:
`SpecId::CANCUN`, Tron chain id, блок-энв (GASPRICE/BASEFEE = energyPrice,
BLOBHASH/BLOBBASEFEE = 0). Существующий стек
(`MultiContractRunner`/`Executor`/`Backend`/cheatcodes/fuzz/invariant)
дженерик по сети и работает без правок.
Ограничения этапа 1 (документируются): CREATE2-адреса по-эфирному (0xff),
precompiles 0x09/0x0a и опкоды 0xD0–0xDF не поддержаны.

**Этап 2 — `tron-revm`:** кастомная таблица инструкций (0xD0–0xDF поверх
journaled state), Tron-таблица precompiles, CREATE2 по формуле Tron (0x41),
energy = FRONTIER gas-таблицы revm + TVM-дельты (**уточнено 2026-07-12** по
java-tron/live golden — НЕ «revm-gas 1:1»; план E: FRONTIER-базис + `override_gas`
дельт, MLOAD/MSTORE/MSTORE8 = SPECIAL_TIER, no-refund, no-EIP-3860; exact-match
на Nile подтверждён на view и write), bandwidth — отдельный проход (размер
protobuf-tx). `--gas-report` показывает две колонки: energy и bandwidth.
**Реализовано (план H, gas-report):** на tron-прогоне (`config.networks.is_tron()`,
run-level) колонка gas переименована в energy (`trace.gas_used` = TVM energy),
добавлен bandwidth-блок (байты) — оценка per-frame через реальные билдеры
provider'а (`build_trigger_raw`/`build_create_raw`, 65-байтовая dummy-подпись) + 64
(java-tron `BandwidthProcessor.consume`), точная для нашего broadcast-пути. EVM-вывод
байт-в-байт не изменён (гейт is_tron + `skip_serializing_if` в JSON). Юниты против
committed-фикстур (trigger → 345, create → 853).

**Гибрид-страховка:**
- `forge test --fork-url <host>/jsonrpc` — fork-режим через частичный `eth_*`
  java-tron; только tip чейна, на исторические блоки — явная ошибка.
  **Реализовано (план G, read-only):** два слоя на нашей стороне —
  nonce-shim (`eth_getTransactionCount → 0x0`, java-tron отдаёт постоянный
  `-32601`) и tip-only unpin state-блока (java-tron `/jsonrpc` отдаёт
  account/storage/code только на TAG `latest`, на номер блока — `-32602`;
  fork-db поэтому НЕ пиним на tron-пути). Live-канал только mainnet
  (`api.trongrid.io/jsonrpc`; на `api.nileex.io` `/jsonrpc` не смонтирован).
  Fork под `forge script` (broadcast) остаётся отклонённым.
  Явная ошибка на исторический `--fork-block-number` — **реализовано (план H, H1):**
  bail «Tron forks are tip-only: state is served only at the chain tip …» в общей
  точке `TestArgs::run_tests` (покрывает `forge test`/`snapshot`/`coverage`) +
  зеркальный гард в чит-коде `vm.createFork`/`createSelectFork` с явным блоком;
- команда-хелпер для поднятия java-tron в докере и интеграционных прогонов
  критичных контрактов перед мейннет-деплоем.

### 4.5 Деплой и RPC (forge script, forge create)

**`tron-provider`:** read — `eth_*` java-tron где есть, иначе HTTP `/wallet/*`
(`triggerconstantcontract` для оценки energy, `getcontract`, `getnowblock`,
`gettransactioninfobyid`); write — только HTTP `/wallet/*`.

**`tron-primitives`:** protobuf-типы (`raw_data`, `TriggerSmartContract`,
`CreateSmartContract`; референс — opentron), txID = sha256(raw_data),
подпись secp256k1 по sha256 (переиспользуется `foundry-wallets`).
TAPOS: перед отправкой — `getnowblock` → `ref_block_bytes/hash`,
`expiration` +60 сек (настраиваемо); при ретрае после истечения транзакция
пересобирается. Nonce-логика не используется.

**Интеграция в forge script** (по образцу Tempo-ветки в
`crates/script/src/lib.rs`): при `network = "tron"` —
симуляция на tron-revm → сборка protobuf → подпись →
`/wallet/broadcasttransaction` → подтверждение поллингом
`gettransactioninfobyid` с ограничением ретраев.
Broadcast-артефакты (`broadcast/*.json`) — тот же формат + Tron-поля
(txID, base58-адреса, fee_limit).
Адрес деплоя вычисляется из txID → в симуляции известен только после сборки
транзакции; для детерминированных адресов — CREATE2 (0x41).

### 4.6 cast

- read-команды (`call`, `balance`, `block`, `logs`, `storage`) → `tron-provider`;
- `cast send` → protobuf-путь;
- новые утилиты: `cast tron-address` (T…/41…/0x…), `cast to-sun`/`from-sun`;
- адресные аргументы принимают все форматы, вывод base58 при `network = "tron"`.

### 4.7 Конфигурация

```toml
[profile.default]
network = "tron"                 # включает Tron-ветку везде
solc = "..."                     # этап 1; этап 2 — авторезолв

[tron]
fee_limit = 1000000000           # SUN (= 1000 TRX), дефолт
origin_energy_limit = 10000000
user_fee_percentage = 100
expiration = 60                  # сек

[rpc_endpoints]
mainnet = "https://api.trongrid.io"
nile = "https://nile.trongrid.io"
shasta = "https://api.shasta.trongrid.io"
local = "http://127.0.0.1:9090"
```

API-ключ TronGrid — через env (header `TRON-PRO-API-KEY`).

### 4.8 Верификация (этап 3)

Новый `VerificationProvider` (`crates/verify/src/provider.rs`) под TRONSCAN API.

**Реализовано (план H):** провайдер `crates/verify/src/tronscan/mod.rs`
(`VerificationProviderType::Tronscan`) — keyless, СИНХРОННЫЙ. `VerifyArgs::run`
ветвится на `config.networks.is_tron()` до alloy chain-резолюции (зеркало
`forge create`), форсит TronScan-провайдера, пропускает aux-Sourcify. Submit —
`multipart/form-data` POST `/api/solidity/contract/verify`: base58-адрес, флаттеный
исходник (`foundry_common::flatten`, без vanilla-solc dry-run и ipfs-ассерта),
`constructorParams`=hex без `0x`, пин `compiler = tron_v<longVersion>`
(`crates/tron/solc/src/pins.rs`), `evmVersion` нормализован против компилятора.
Маппинг `data.status`: 2006=успех, 2001=already verified, 2007/2008=fail;
`check`/`--watch` — re-query `/api/solidity/contract/info` (`status==2`). Host-routing
по chain id: mainnet 728126428 → `apilist.tronscanapi.com`, Nile 3448148188 →
`nileapi.tronscan.org`, оверрайд `--verifier-url`. Доступно как standalone
`forge verify-contract` (адрес `T…`/`41…`/`0x…`) и embedded `forge create --verify`
(с pre-broadcast preflight до траты TRX). `forge script --verify` — пока явный bail
(шов асимметричен, follow-up). Live acceptance-гейт на Nile пройден
(`tron_v0.8.27+commit.19164bed`, `/info` status 2).

## 5. Вне скоупа (YAGNI)

- anvil-tron (локальная нода): тесты покрывает tron-revm, интеграцию — java-tron
  в докере (оркестрация готовой командой);
- chisel, TUI-дебаггер под Tron-опкоды;
- TRC-10-опкоды в этапе 1;
- исторический fork-режим (ограничение java-tron);
- shielded-TRC20 precompiles (заглушки).

## 6. Этапность

**Этап 1 — пайплайн (ранняя польза для миграции):**
tron-primitives (адреса, protobuf, подпись) → tron-provider →
`NetworkVariant::Tron` + `TronEvmNetwork` (наивный) → forge build (drop-in
tron-solc) → cast read/send → forge script/деплой на Nile.
Результат: полный цикл build → test (наивный) → deploy без докера.

**Этап 2 — точность:**
tron-revm (опкоды, precompiles, CREATE2, energy/bandwidth) → gas-report →
резолвер tron-solc → fork-режим → golden-тесты против Nile в CI.

**Этап 3 — полировка/OSS:**
верификация TRONSCAN → документация → бинарные релизы → вынос tron-крейтов.

## 7. Тестирование самого форка

- юнит-тесты кодека адресов и protobuf против известных векторов
  (реальные транзакции mainnet, эталонные пары T…/41…);
- golden-тесты соответствия семантик: компиляция → деплой на Nile → вызовы →
  сверка результатов/energy с revm-симуляцией (CI-страховка);
- существующие тест-сьюты Foundry должны оставаться зелёными для
  Ethereum/Optimism/Tempo-путей (не ломаем базу).

## 8. Основные риски

| Риск | Митигание |
|---|---|
| via-ir не поддержан tron-solc | Проверка на первой неделе; fallback — legacy codegen |
| Нет нативных бинарников tron-solc под macOS/arm | Сборка из исходников, свой хостинг |
| Противоречивые доки по precompiles/опкодам | Сверка с исходниками java-tron до реализации tron-revm |
| Расхождение revm-семантики с mainnet | Golden-тесты против Nile; гибрид с java-tron |
| Rebase-стоимость (двойной upstream: tempoxyz ← foundry-rs) | Минимальный дифф в существующих crates, вся Tron-логика в отдельных крейтах |
| TAPOS/expiration при медленном бродкасте больших скриптов | Пересборка транзакции при ретрае, настраиваемый expiration |
