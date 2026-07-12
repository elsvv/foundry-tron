# foundry-tron: статус проекта

Форк Foundry с поддержкой Tron (TVM). Fork: `elsvv/foundry-tron`, upstream: `foundry-rs/foundry` (в апстриме уже есть мультисетевость Ethereum/Optimism/Tempo — Tron добавляется по тем же швам). Дефолтная ветка форка — **`tron-dev`**. Текущая работа (планы C2 → D) идёт в ветке **`tron-dev-continue`** (ответвлена от `tron-dev`); PR `tron-dev-continue` → `tron-dev` в процессе.

Локальный путь на этой машине: `/Users/andrey/vibe_projects/foundry-tron` — это САМ репозиторий (плоская структура: `crates/`, `docs/tron/`, `sandbox/` прямо в корне). Никакой обёрточной папки/вложенного `foundry/` больше нет — если видите путь вида `.../foundry-tron/foundry/...`, это устаревшее упоминание из старой сессии.

Обновлено: 2026-07-12.

## Документы

- Спека (утверждена): `docs/tron/specs/2026-07-10-foundry-tron-fork-design.md`
- Планы: `docs/tron/plans/` — один план на подсистему, исполняются последовательно.

## Состояние этапов

| План | Что | Статус |
|---|---|---|
| A | `crates/tron/primitives` — адресный кодек base58check/0x41, protobuf-транзакции (txID=sha256(raw_data)), TAPOS, подпись secp256k1 (65 байт, v=27+recid) | ✅ Готов. 18 тестов. Смоук на Nile: tx `3a4d9c5f…` в блоке 69090417 |
| B | `crates/tron/provider` — async HTTP-клиент `/wallet/*`: блоки/TAPOS, балансы, tx-info, constant-вызовы + energy, broadcast, `send_transfer` | ✅ Готов. 15 тестов (оффлайн fixtures + live `TRON_LIVE=1`), E2E на Nile |
| C | `NetworkVariant::Tron`, маркер `TronEvmNetwork` (Network=Ethereum, EvmFactory=`TronEvmFactory`), диспетчеризация forge test, clippy-чистка | ✅ Полностью готов. E2E-гейт (Задача 4) закрыт через C2 |
| C2 | Мини tron-revm: `TronEvmFactory` (EthEvmFactory + insert_instruction для 0xD0–0xD4), закрытие E2E-гейта плана C | ✅ Готов. Задача 1 (`TronEvmFactory`, unit-тесты на реальном tron-solc-байткоде). Задача 2 — sandbox `forge build`+`forge test` 4/4 на байткоде tron-solc (`testIncrement`, `testTronChainId`, `testTransientStorageCancun`, `testNonPayableGuardWithTvmOpcodes`) |
| D | cast/forge script/forge create: деплой и вызовы на Nile через tron-provider | ✅ Готов. Полный сквозной цикл Этапа 1 воспроизведён на Nile (см. ниже) |
| Этап 2 | Полный tron-revm: precompiles (0x09 BatchValidateSign, Ripemd160→0x20003, Blake2F→0x20009), CREATE2-префикс 0x41, energy/bandwidth-репорт, резолвер tron-solc | ⏳ Следующий |

## План D — что работает (сквозной цикл Этапа 1 на Nile)

Все команды диспетчеризуются явно по `network = "tron"` в `foundry.toml` (никакого инференса из chain id / fork). Ветка Tron всегда идёт до alloy-провайдера/`get_chain_id`; запись — через protobuf `/wallet/*`, не `eth_sendRawTransaction`.

- **cast**: `to-sun`/`from-sun`/`tron-address` (оффлайн-утилиты), `call` (constant через `triggerconstantcontract`), `balance` (`--ether` печатает TRX), `send` (native transfer / `trigger` / `--create` деплой). Адреса принимаются в `T…`/`41…`/`0x…`.
- **forge script `--broadcast`**: `prepare_bundled::<TronEvmNetwork>` симулирует локально, фаза-2 fork-симуляции пропускается, `broadcast_tron` шлёт каждую tx protobuf'ом. Артефакт `broadcast/<script>/3448148188/run-latest.json` — стандартной формы, `hash` = txID, плюс опц. блок `tron { txid, ownerBase58, contractAddressBase58, feeLimit, energyUsed, feeSun }` (EVM-фикстуры не ломаются: `#[serde(default, skip_serializing_if)]`).
- **forge create**: Tron-ветка первой в `CreateArgs::run`; компиляция/линковка как у generic, затем `deploy_contract`. `--verify`/`--unlocked`/`--browser` — явные ошибки «not supported on tron yet»; `--fork-url` в forge script — Stage-2 ошибка.
- **fee_limit/expiration**: из `[tron]` конфига, перекрываются `--tron.fee-limit`/`--tron.expiration`.

**Live-подтверждения (Nile, chain id 3448148188, 2026-07-12), sandbox `tron-counter`:** `forge build` (tron-solc) → `forge test` 4/4 (на tron-solc-байткоде) →
- `forge create Counter` → `TEsgXDHsYDMvdpoAswPuPDeghuUiAucivJ`, txID `6a1b82bc…b51fbb` (10.96 TRX);
- `cast send setNumber(7)` txID `d73d8c93…491fb6`; `cast call number()` → 7;
- `cast send` перевод 1 TRX txID `059bedb7…4870d26`; `cast balance --ether` → 1889.18 TRX;
- `forge script Deploy --broadcast` → CREATE `THhVv6vwHsyaKm42hPUHVc8szHy4xNPagt` txID `2507a88a…3292b1` + CALL setNumber(42) txID `cdc904f3…d0d12f`; `number()` → 42; артефакт с txID и `contractAddressBase58` записан.

Известное упрощение: публичный `nile.trongrid.io` без API-ключа WAF-режет всплеск `/wallet/*` POST'ов (HTTP 405) — снимается `TRON_PRO_API_KEY`. Латентные «наивные» арки (`args.rs` DecodeTransaction, `da_estimate.rs`) и library-предеплои/CREATE2/energy-репорт — Этап 2 (см. «Вне скоупа» плана D).

## Ключевые находки (не потерять)

0. **План E / Задача 1 (energy-модель) — известное ограничение call depth 64.** Tron `MAX_DEPTH=64` (`Program.java`), revm — 1024 (`CALL_STACK_LIMIT`, `pub const` в `revm-primitives`, читается напрямую во `frame.rs:175,287` дефолтного хендлера, НЕ поле конфига). Переопределение потребовало бы форка `make_call_frame`/`make_create_frame` — вне скоупа energy-модели. Контракты с рекурсией глубже 64 проходят локально, но ревертят on-chain. Зафиксировано в `crates/evm/core/src/evm/tron/energy.rs`. Также в Задаче 1: refund'ов у Tron НЕТ (`ProgramResult.java:25,221-230` — `futureRefund`/`addFutureRefund`/`getFutureRefund` закомментированы; `Program.java:1272-1278`), поэтому все refund-GasId'ы (sstore_clearing/set/reset, selfdestruct) занулены. **63/64-правило (EIP-150) на Tron НЕ действует** (в отличие от depth-64, которое оставлено ограничением): `Program.getCallEnergy`/`getCreateEnergy` (`:1834-1848`) режут 1/64 только под `allowTvmCompatibleEvm && contractVersion==1` (выкл. на mainnet/Nile) — дефолт форвардит `min(requested, всё доступное)`. Реализовано override'ом `GasId::call_stipend_reduction = u64::MAX`, так что `gas_limit - gas_limit/u64::MAX == gas_limit` (вектор `inner_create_forwards_all_gas`). **EIP-3860 (per-word initcode-метринг) тоже отключён**: `initcode_per_word` в базовой таблице = 2 для всех спеков и метрится внутренним CREATE / стоковым CREATE2, тогда как `EnergyCost.getCreateCost` (:414-419) per-word-слагаемого не имеет — занулён override'ом `GasId::initcode_per_word = 0` (вектор `inner_create_has_no_eip3860_metering`; `limit_contract_initcode_size` снимает только size-cap).
1. **tron-solc вставляет TVM-опкоды `0xD3`/`0xD2` (CALLTOKENID/CALLTOKENVALUE) в non-payable guard каждого контракта** → байткод tron-solc не исполняется на ванильном revm (`OpcodeNotFound` в конструкторе). Флага отключения нет. Отсюда план C2.
2. **Нативные бинарники tron-solc существуют**: github.com/tronprotocol/solidity/releases, ассет `solc-macos` (Intel, на Apple Silicon — через Rosetta 2), версии до 0.8.27_Democritus_v4.8.1. Скачан в `~/.foundry-tron/solc/tron-solc-0.8.27`. TronBox качает только wasm — нативные лежат именно в релизах.
3. Chain id Tron mainnet: `728126428`. TVM ≈ Cancun (java-tron 4.8.x): PUSH0, TLOAD/TSTORE, MCOPY есть; BLOBHASH/BLOBBASEFEE — заглушки 0.
4. Транзакции — protobuf (не RLP), txID = sha256(raw_data), подпись по txID, TAPOS вместо nonce, `eth_sendRawTransaction` отсутствует — запись только через HTTP `/wallet/*` (см. crates/tron/provider).
5. Sample-проект: `sandbox/tron-counter` (без forge-std; ассерты chainid=728126428 и tstore/tload).
6. **ISCONTRACT (0xD4) в java-tron проверяет наличие контракт-аккаунта (`ContractCapsule`), а не непустоту кода** (финальное ревью C2 по исходникам @develop). Заглушка этапа 1.5 (`load_account_code` непуст) расходится только в экзотике: self-check внутри конструктора (java-tron уже true, у нас ещё false), контракт с пустым runtime-кодом. Учесть при golden-тестах против Nile в Этапе 2. Там же: трейсы forge пока показывают 0xD0–0xD4 как unknown-мнемоники (косметика, Этап 2/3).
7. **Адрес контракта = `keccak256(txID ‖ owner21)[12..]`**, где `txID = sha256(raw_data)` (32 байта), `owner21 = 0x41 ‖ owner20`. Источник — java-tron `chainbase/.../WalletUtil.java:39-51` (@develop, `generateContractAddress`): `Hash.sha3omit12(txRawDataHash ++ ownerAddress)`. НЕ «keccak(txID)», НЕ RLP (обе гипотезы разведки были неверны). Проверено live на трёх реальных деплоях Nile — локально вычисленный адрес совпал с `TxInfo.contract_address` ноды (в `deploy_contract`/`broadcast_tron` это hard-assert): `6a1b82bc…`→`TEsgX…`, `2507a88a…`→`THhVv…`.
8. **Nile chain id: `3448148188` (0xcd8690dc)** — проверено live через `eth_chainId` и по путям артефактов `broadcast/<script>/3448148188/`. Mainnet — `728126428` (0x2b6653dc).
9. **Уточнение спеки: broadcast остаётся на `/wallet/broadcasthex`.** Спека §4.5 упоминает `/wallet/broadcasttransaction`; это эквивалент (тот же protobuf, JSON- vs hex-обёртка), миграция не нужна — `broadcasthex` покрыт live-тестами и работает на всех путях D. Подпись — 65 байт `r‖s‖(27+recid)` по `sha256(raw_data)`; никакого EIP-155/chain id в подписи (replay-защита — TAPOS+expiration). `fee_limit` — поле `TransactionRaw` (tag 18, SUN); `origin_energy_limit`/`consume_user_resource_percent` — поля `SmartContract` (tags 8/6): два разных уровня протобуфа.

## Окружение (важно для любой машины)

- **Тулчейн:** на текущей машине (`andrey`) — свежий rustup (stable 1.97.0 по умолчанию, nightly с rustfmt). Обычный `cargo` работает; перед командами достаточно `export PATH="$HOME/.cargo/bin:$PATH"`. Форматирование — `cargo +nightly fmt` (repo `rustfmt.toml`).
  Зависимости требуют rustc ≥1.91. Fallback для машин, где системный/Homebrew rustc старее и перекрывает rustup (как на исходной машине с Homebrew 1.88): все cargo-команды через явный путь к stable-тулчейну:
  ```bash
  TC=$(dirname "$(rustup which --toolchain stable cargo)"); PATH="$TC:$PATH" cargo <...>
  ```
- **tron-solc:** нативный бинарник `0.8.27` — `/Users/andrey/.foundry-tron/solc/tron-solc-0.8.27` (universal macOS, sha256 `9e369b44…c7ce17aa`; путь абсолютный, т.к. `SolcReq::Local` не разворачивает `~`). `sandbox/tron-counter/foundry.toml` указывает `solc` именно на него.
- **Live-тесты:** `TRON_LIVE=1` + `TRON_PRIVATE_KEY` (файл `.env.tron-dev` в корне репо, в git НЕ входит — перенести вручную или сгенерировать новый ключ и пополнить через кран https://nileex.io/join/getJoinPage). Текущий тестовый адрес: `TX7izXWcmofRYonzdcThrS78jifMtVWCuf` (~1997 TRX на Nile).
- Тесты tron-крейтов: `cargo test -p foundry-tron-primitives -p foundry-tron-provider`. CI-линт: `cargo clippy --all-targets` с `-Dwarnings` — tron-крейты чистые.

## Процесс работы (утверждено пользователем)

- Планы исполняются dynamic-workflow: исполнители Opus → Opus-верификатор после каждой задачи (fix-цикл до 3) → Fable-ревью после каждых двух задач и в конце (тоже с fix-циклом). Шаблон скрипта — в планах/истории сессий.
- Требования к тестам: только реальные векторы/fixtures (сеть, официальные .proto, java-tron), никакой тавтологии, запрещено ослаблять ассерты и ставить `#[ignore]`; live-тесты гейтятся env-переменной с явным `eprintln("skipped…")`.
- Верификаторы обязаны независимо перепроверять доменные факты (повторные curl, пересчёт векторов, сверка с первоисточниками) — этот паттерн уже поймал 2 неверных вектора в планах и находку №1.
