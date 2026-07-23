# foundry-tron: статус проекта

Форк Foundry с поддержкой Tron (TVM). Fork: `elsvv/foundry-tron`, upstream: `foundry-rs/foundry` (в апстриме уже есть мультисетевость Ethereum/Optimism/Tempo — Tron добавляется по тем же швам). Дефолтная ветка форка — **`tron-dev`**. Планы C2 → D — в ветке **`tron-dev-continue`** (ответвлена от `tron-dev`). Ядро Этапа 2 (план E: energy/precompiles/CREATE2) — в ветке **`tron-stage2`** (стек поверх `tron-dev-continue`); PR `tron-stage2` → `tron-dev-continue`.

Локальный путь на этой машине: `/Users/andrey/vibe_projects/foundry-tron` — это САМ репозиторий (плоская структура: `crates/`, `docs/tron/`, `sandbox/` прямо в корне). Никакой обёрточной папки/вложенного `foundry/` больше нет — если видите путь вида `.../foundry-tron/foundry/...`, это устаревшее упоминание из старой сессии.

Обновлено: 2026-07-23. Этап 4 (план I, «fidelity») — в ветке **`tron-stage4`** (от `master`).

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
| E | Ядро tron-revm (ветка `tron-stage2`): faithful energy-модель (FRONTIER-таблицы + TVM-дельты), полный набор precompiles java-tron, CREATE2 по формуле Tron + cheatcode | ✅ Готов. Golden energy exact-match на Nile (view+write), CREATE2/precompile golden подтверждены (см. «План E» ниже) |
| F | Резолвер tron-solc (крейт `foundry-tron-solc`, GitHub-релизы + чексуммы, шов `Config::ensure_solc`) | ✅ Готов. Крейт с pinned sha256 (0.8.25/26/27 × linux/macos/windows), автовыбор в `ensure_solc` при `network=tron`, sandbox без абсолютного пути. Оффлайн-тесты зелёные, download-гейт `TRON_SOLC_DOWNLOAD=1` |
| G | Fork-режим (`forge test --fork-url` через `/jsonrpc`) с постоянным shim `eth_getTransactionCount → 0x0` (java-tron отдаёт `-32601`) + tip-only state-читка (`/jsonrpc` берёт state только на `latest`) | ✅ Готов. Nonce-shim (G1) + unpin state-блока для tron (G2). Live read-only mainnet fork на USDT зелёный (см. «План G» ниже) |
| H | Этап 3 «полировка/OSS» (ветка `tron-stage3`): gas-report energy+bandwidth (H2), fork-guard на явный исторический блок (H1), TRONSCAN verify — `forge verify-contract` + embedded `forge create --verify` (H3/H4), USER_GUIDE/README/version-stamp (H5), tron-live CI workflow (H6) | ✅ Готов. H1–H6 (см. «План H» ниже). Live acceptance-гейт verify E2E пройден на Nile |
| I | Этап 4 «fidelity» (ветка `tron-stage4`): precompile-кламп = ровно java-tron (I1), value-only вызов контракта через TriggerSmartContract (I2), tron-solc 0.8.28 + динамический резолв из solc-bin (I3), chainid-дефолт/BASEFEE-env/no-op-warn (I4), getchainparameters (I5), energy_penalty/estimateenergy/`cast estimate`/fee_limit-валидация (I6), TIP-491 penalty в gas-report + фикс Deployment Energy (I7), base58 в трейсах (I8), release prep (I9) | ✅ Готов. Оффлайн-юниты зелёные; live-гейты `TRON_LIVE=1` (mainnet read-only / Nile spend) и `TRON_SOLC_DOWNLOAD=1` — см. «План I» ниже. Тулчейн-версия штампа `--version` → `0.2.0` |

## План D — что работает (сквозной цикл Этапа 1 на Nile)

Все команды диспетчеризуются явно по `network = "tron"` в `foundry.toml` (никакого инференса из chain id / fork). Ветка Tron всегда идёт до alloy-провайдера/`get_chain_id`; запись — через protobuf `/wallet/*`, не `eth_sendRawTransaction`.

- **cast**: `to-sun`/`from-sun`/`tron-address` (оффлайн-утилиты), `call` (constant через `triggerconstantcontract`), `balance` (`--ether` печатает TRX), `send` (native transfer / `trigger` / `--create` деплой). Адреса принимаются в `T…`/`41…`/`0x…`.
- **forge script `--broadcast`**: `prepare_bundled::<TronEvmNetwork>` симулирует локально, фаза-2 fork-симуляции пропускается, `broadcast_tron` шлёт каждую tx protobuf'ом. Артефакт `broadcast/<script>/3448148188/run-latest.json` — стандартной формы, `hash` = txID, плюс опц. блок `tron { txid, ownerBase58, contractAddressBase58, feeLimit, energyUsed, feeSun }` (EVM-фикстуры не ломаются: `#[serde(default, skip_serializing_if)]`).
- **forge create**: Tron-ветка первой в `CreateArgs::run`; компиляция/линковка как у generic, затем `deploy_contract`. **`--verify` реализован (план H, H4):** после деплоя контракт верифицируется через TronScan-провайдера (деплой + верификация одной командой `forge create --verify`), с pre-broadcast preflight (валидирует compiler-пин/хост/флаттен ДО траты TRX, зеркало generic). `--unlocked`/`--browser` остаются явными ошибками «not supported on tron yet»; `--fork-url` в forge script — Stage-2 ошибка.
- **forge script `--verify`** — **пока НЕ поддержан на tron: явная ошибка (план H, H4).** Причина (шов НЕ ложится зеркально `forge create`): скриптовая верификация идёт через etherscan-центричный `verify_contracts`/`VerifyBundle` (гейт `verify.etherscan.has_key() || effective_type() != Etherscan`, а tron-ветка `run` вообще не зовёт `broadcasted.verify()`). Разблокировка потребовала бы форсить TronScan-провайдера в `VerifyBundle`, добавить вызов `verify()` в tron-ветку и скипнуть generic-preflight — не проверяемо live без траты TRX в рамках H4. **Follow-up (DEFER):** развести `verify_contracts` на TronScan-провайдера, покрыть live-E2E (Nile). Standalone `forge verify-contract`/`forge create --verify` уже покрывают эту потребность (оба маршрутизируются в TronScan).
- **fee_limit/expiration**: из `[tron]` конфига, перекрываются `--tron.fee-limit`/`--tron.expiration`.

**Live-подтверждения (Nile, chain id 3448148188, 2026-07-12), sandbox `tron-counter`:** `forge build` (tron-solc) → `forge test` 4/4 (на tron-solc-байткоде) →
- `forge create Counter` → `TEsgXDHsYDMvdpoAswPuPDeghuUiAucivJ`, txID `6a1b82bc…b51fbb` (10.96 TRX);
- `cast send setNumber(7)` txID `d73d8c93…491fb6`; `cast call number()` → 7;
- `cast send` перевод 1 TRX txID `059bedb7…4870d26`; `cast balance --ether` → 1889.18 TRX;
- `forge script Deploy --broadcast` → CREATE `THhVv6vwHsyaKm42hPUHVc8szHy4xNPagt` txID `2507a88a…3292b1` + CALL setNumber(42) txID `cdc904f3…d0d12f`; `number()` → 42; артефакт с txID и `contractAddressBase58` записан.

Известное упрощение: публичный `nile.trongrid.io` без API-ключа WAF-режет всплеск `/wallet/*` POST'ов (HTTP 405) — снимается `TRON_PRO_API_KEY`. Латентные «наивные» арки (`args.rs` DecodeTransaction, `da_estimate.rs`) и library-предеплои/CREATE2/energy-репорт — Этап 2 (см. «Вне скоупа» плана D).

## План E — ядро tron-revm (ветка `tron-stage2`)

Превращение наивного `TronEvmFactory` (этап 1.5: только 0xD0–0xD4) в верный tron-revm. Всё внутри `crates/evm/core/src/evm/tron/{mod.rs,energy.rs,precompiles.rs,create.rs}`; foundry-путь и raw-путь покрыты одним швом `inject_tron_extensions` (оба `create_evm`/`create_evm_with_inspector`).

1. **Energy-модель (`energy.rs`).** Tron = pre-EIP-150 (FRONTIER) Ethereum-gas + TVM-дельты, реализовано как подмена ДАННЫХ (не логики) поверх CANCUN-исполнения: `*gas_table_mut() = gas_table()` (FRONTIER-базис), затем `insert_gas` TVM-дельт, `set_gas_params(GasParams::new_spec(FRONTIER))` + `override_gas` шести GasId (EXP-byte 10, refunds→0, intrinsic→0, selfdestruct new-account 25000, initcode_per_word 0, 63/64→off), `limit_contract_code_size/initcode_size = MAX` (нет EIP-170/3860). Опкоды 0xD0–0xD4 и блочные стабы (DIFFICULTY/GASLIMIT→0, BASEFEE→100, GASPRICE→0, BLOBHASH/BLOBBASEFEE→0) — `insert_instruction`.
2. **Precompiles (`precompiles.rs`).** Таблица java-tron через `extend_precompiles`: 0x03 = двойной sha256 (НЕ ripemd160), 0x05 ModExp (EIP-198 divisor 20, без 200-floor), 0x09 BatchValidateSign (TIP-43, замещает blake2f), 0x0a ValidateMultiSign (TIP-60, замещает KZG), настоящий ripemd160→0x020003, blake2f→0x020009, стабы shielded/vote/FreezeV2. Крипто-ядра переиспользованы из revm-precompile (без новых зависимостей). Ошибки входа → halt/revert (не fatal `PrecompileError`). Labels — `TRON_PRECOMPILES` в evm/networks.
3. **CREATE2 (`create.rs`).** Кастомная инструкция 0xF5 → `CreateScheme::Custom{addr}`, где `addr = keccak256(0x41 ‖ sender20 ‖ salt ‖ keccak256(initcode))[12..]` (`foundry_tron_primitives::address::create2_address`). Cheatcode `computeCreate2Address*` специализирован на Tron. CREATE (внутренний) остаётся EVM-схемой — документированная локальная дельта (Tron-схема `keccak(rootTxId‖nonce)` невоспроизводима локально).

**Golden против Nile (fresh Counter, factor=1, exact-match, ветка tron-stage2):**
- READ `number()` — локальная симуляция `TronEvmFactory` == нода: **414 energy** (deploy tx `fcfc5880…aafd9e67`, контракт `TJFRVxUW3wU7RGiSif9fANRX51n8MG9qr4`);
- WRITE `setNumber(7)` — локально == нода: **20438 energy** (tx `a34106ea…f183db2f`);
- CREATE2-адрес и 0x01/0x03 precompile-выходы подтверждены live отдельными golden-тестами.

Тесты: `cargo test -p foundry-evm-core tron` = 38 зелёных; golden в `crates/tron/provider/src/client.rs` (`live_golden_energy_parity_on_nile`, гейт `TRON_LIVE`).

## План F — авторезолв tron-solc (крейт `foundry-tron-solc` + шов `Config::ensure_solc`)

svm непригоден (`SOLC_RELEASES_URL` захардкожен на `binaries.soliditylang.org`, `verify_checksum` сверяет с soliditylang-листом → гарантированный mismatch для tron-бинарников). Решение — отдельный крейт `crates/tron/solc` (не в config: там нет reqwest/sha2/tokio), качающий нативные бинарники из GitHub-релизов `tronprotocol/solidity` и сверяющий sha256 по встроенной pinned-таблице.

1. **Крейт `foundry-tron-solc` (F1).** `resolve_tron_solc(version, offline) -> PathBuf`: кэш-проверка (`~/.foundry-tron/solc/tron-solc-{ver}` + sha256) → скачивание (reqwest, redirect-friendly, таймауты, ретраи ×3, `RuntimeOrHandle`-паттерн для блокирующего HTTP) → sha256-верификация → атомарная запись (tmp+rename, chmod 755). Pinned sha256 в `pins.rs` (`const`, источник `tronprotocol.github.io/solc-bin/{platform}/list.json`, дата 2026-07-12): 0.8.25/0.8.26/0.8.27 × {linux-amd64, macosx-amd64, windows-amd64}. НИКОГДА не качает при `offline` или кэш-хите; mismatch = hard error (не тихий фолбэк). Оффлайн-юниты + download-гейт `TRON_SOLC_DOWNLOAD=1` (качает самый малый ассет — windows ~10 МБ — и сверяет пин). **Конвенция `~/.foundry-tron/solc/tron-solc-{ver}` кодифицирована здесь** (`binary_path`).
2. **Интеграция в config (F2).** `Config::ensure_solc` при `self.networks.is_tron()` и `solc` = `None`/`Version(v)`: выбор версии (None → `default_version()`=0.8.27; Version(v) → v) → `resolve_tron_solc(&v, self.offline)` → `Solc::new_with_version(path, v)` (без exec `--version`, без `verify_checksum`). `SolcReq::Local` — как раньше (явный оверрайд, падает в общую ветку, резолвер не трогается). Non-tron путь не изменён. Диспетч не ломается: `ensure_solc` теперь отдаёт `Some(Solc)` → `SolcCompiler::Specific` (не AutoDetect).
3. **Sandbox без абсолютного пути.** `sandbox/tron-counter/foundry.toml` больше НЕ несёт `solc = "/Users/…"` — резолвится авто в машинный кэш. `forge build` + `forge test` = 5/5 через авторезолв (кэш-хит).

**Известный gap:** нативного Linux-ARM бинарника tron-solc нет (`linux-arm64`/`macosx-aarch64` в solc-bin отдают 404) — `Platform::LinuxAarch64` → понятная ошибка `UnsupportedPlatform` (build-from-source). macOS ARM покрыт universal `solc-macos` под ключом `macosx-amd64`.

Тесты: `cargo test -p foundry-tron-solc` (оффлайн + гейт); `cargo test -p foundry-config tron` = 8 зелёных (4 новых config-теста: авторезолв-дефолт из кэша, honors explicit Local, Version-арм → резолвер (unknown-версия → NoPin, детерминированно без сети), non-tron не изменился).

## План G — read-only fork через `/jsonrpc` (ветка `tron-stage2`)

`forge test --fork-url <host>/jsonrpc` при `network = "tron"` теперь форкает реальный state Tron mainnet в tron-revm (energy-модель + precompiles плана E). **Live-канал только mainnet** (`https://api.trongrid.io/jsonrpc`): на `api.nileex.io` `/jsonrpc` НЕ смонтирован (nginx 404), `nile.trongrid.io` с этой машины недоступен. Read-only: TRX не тратится.

Два слоя на нашей стороне (foundry-fork-db git-pinned — не трогаем):

1. **Nonce-shim (G1).** `TronNonceShimLayer` (tower-layer, `crates/common/src/provider/mod.rs`) короткозамыкает `eth_getTransactionCount` → `"0x0"` без обращения к сети. java-tron отдаёт по этому методу постоянный `-32601` (hard-stub `TronJsonRpc.java`), а fork-db грузит аккаунт через `try_join3(balance, nonce, code)` → любой account-fetch падал. Nonce=0 корректен для Tron (нет EVM-CREATE-nonce; foundry инкрементит свой in-memory от базы). Ставится строго на tron-fork-пути: `EvmOpts::fork_provider_with_url` → `.tron_shim(self.networks.is_tron())` (`opts.rs`), покрывает и `get_fork` (`--fork-url`), и `MultiFork::create_fork` (`vm.createSelectFork`). Понятная ошибка на неверном эндпоинте (дали `/wallet/*`-хост): `fork_evm_env` добавляет hint «tron fork requires the /jsonrpc endpoint».
2. **Tip-only state-читка (G2, находка live-тестов).** java-tron `/jsonrpc` отдаёт account/storage/code **только на TAG `latest`** и отвечает `-32602 "QUANTITY not supported, just support TAG as latest"` на конкретный номер блока. foundry-fork-db же пинит state-запросы на номер fork-блока → каждый account-load падал даже с nonce-shim'ом. Фикс — в `MultiFork::create_fork` (наш код, `evm/core/src/fork/multi.rs`) для tron НЕ пиним state-блок (`SharedBackend::new(provider, db, None)`; fork-db тогда шлёт `latest`), block-env остаётся пиненым на `number`. Tron-fork поэтому tip-only (state — на текущем tip, а не на historical блоке) — это фундаментальное ограничение `/jsonrpc`, спека §4.4 его и предполагала («только tip чейна»). Non-tron сети пин сохраняют (`(!is_tron).then(|| number.into())`).

**Разведка ошибалась в одном:** scout-репорт утверждал, что `getBalance/getStorageAt/getCode` «работают live» — но он пробовал их с TAG `latest`; fork-backend шлёт QUANTITY (номер блока), и java-tron его режет. G1-nonce-shim'а в одиночку НЕ хватило; понадобился второй слой (unpin). Поймано только live-прогоном USDT-теста.

`forge script` fork остаётся отклонённым (broadcast-контекст, вне скоупа): при `--fork-block-number` — `bail` «forking a Tron node is not supported under forge script; use `forge test --fork-url …/jsonrpc` for a read-only fork». `forge test` без `--fork-url` (оффлайн-сьюты) не задет.

**Follow-up (emergent-находка финального ревью F+G, 2026-07-12) — ✅ ЗАКРЫТ планом H (H1, 2026-07-14; hotfix H1 после ревью, 2026-07-14):** ранее `forge test/snapshot/coverage --fork-url …/jsonrpc --fork-block-number <исторический>` на tron проходил МОЛЧА (block env пинился на указанный номер, а state из-за tip-only ограничения читался на текущем tip — смесь), тогда как спека §4.4 обещает «на исторические блоки — явная ошибка» (универсально, не только под `forge test`). Реализован **bail** (утверждённое решение ревью F+G + скаута): при явном историческом `--fork-block-number` на tron — понятная ошибка «Tron forks are tip-only: state is served only at the chain tip (/jsonrpc serves state only at TAG latest). Drop --fork-block-number for a tip fork.». Ключевание — на СЫРОМ пользовательском значении `evm_opts.fork_block_number` ДО авто-пина `EvmOpts::get_fork` (`opts.rs:368` пинит `.or(latest)`). **Гард стоит в общей точке-схождении `TestArgs::run_tests` (до `infer_network_from_fork`, `crates/forge/src/cmd/test/mod.rs`) — через неё проходят ВСЕ выполняющие тесты команды (`forge test`, `forge snapshot`, `forge coverage`), так что молчаливой смеси не может пройти ни один путь.** Первая реализация H1 ставила гард только в `compile_project`/`compile_and_run_brutalized`, из-за чего `forge coverage` (собственный `self.build()` + прямой вызов `run_tests`, `crates/forge/src/cmd/coverage.rs`) обходил его и падал на TCP-connection, а не на tip-only ошибке — это hotfix'ом закрыто переносом гарда в `run_tests` + fail-fast дубликат в `CoverageArgs::run`. Пер-командные fail-fast сайты (`compile_project`, `compile_and_run_brutalized`, `CoverageArgs::run`) вызывают гард до компиляции/сети, чтобы ошибка всплыла без запуска `tron-solc`. Зеркальный гард в чит-коде: `vm.createFork`/`vm.createSelectFork(url, blockNumber)` (`_1`-варианты, `block.is_some()`) на tron → ошибка чит-кода с тем же объяснением (`crates/cheatcodes/src/evm/fork.rs`, `ensure_tron_tip_only_fork` в `create_fork_request`); безблочные и at-transaction варианты не задеты. G2-unpin (`multi.rs`) и Stage-1 отказ `forge script` fork НЕ тронуты. Тесты: оффлайн CLI-тесты `tron_historical_fork_block_bails` и `tron_historical_coverage_fork_block_bails` (`crates/forge/tests/cli/tron.rs`, bail до сети/tron-solc — снапшот stderr, покрывают test- и coverage-пути) + юнит `tron_tip_only_fork_guard` (`fork.rs`, матрица is_tron × block); tip-fork без блока остаётся зелёным (`tron_mainnet_fork_reads_usdt` не задет — `--fork-block-number` не передаёт). Спека §4.4 менять не нужно — формулировка уже обещала явную ошибку.

**Live-подтверждение (mainnet, chain id 728126428, 2026-07-12), USDT `TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t` (0x41-снятый `a614f803b6fd780986a42c78ec9c7f77e6ded13c`):** `name()` == "Tether USD", `symbol()` == "USDT", `decimals()` == 6, `totalSupply()` = 90290450587295290 (>0), `balanceOf(TWd4WrZ9wn84f5x1hZhL4DHvk738ns5jwb)` = 538852373717000 (>0), `extcodesize` = 14945, storage-слот балансов `keccak256(abi.encode(holder,0))` == `balanceOf` (кросс-чек call-пути и storage-пути), energy `name()` под tron-моделью = 1915 (тронский масштаб; warm-повтор коммитного теста, воспроизводимо; cold=1924/warm=1909 по отдельной пробе). Ассерты: константы точные, агрегаты `> 0`.

Тест: `crates/forge/tests/cli/tron.rs` (`tron_mainnet_fork_reads_usdt`, гейт `TRON_LIVE=1`, обе fork-конструкции: `--fork-url` и `vm.createSelectFork`); юнит-шим — `crates/common/src/provider/mod.rs` (`tron_shim_intercepts_get_transaction_count`, `tron_shim_passes_other_methods_through`).

## План H — Этап 3 «полировка/OSS» (ветка `tron-stage3`)

Закрывает хвосты Этапа 2 (спека §4.4: gas-report energy+bandwidth; follow-up ревью
F+G: молчаливый исторический fork-блок) и Этап 3 спеки §4.8/§6 (верификация TRONSCAN
→ документация → CI). Ветка `tron-stage3` от `tron-dev`. Порядок H1→H6, каждая —
отдельный конвенциональный коммит; Fable-ревью после H1+H2, H3+H4 и в конце этапа.

**H1 — fork-guard на явный исторический блок.** Раньше `forge test/snapshot/coverage
--fork-url …/jsonrpc --fork-block-number <исторический>` на tron проходил МОЛЧА
(block env пинился на номер, а state из-за tip-only читался на текущем tip — смесь),
хотя спека §4.4 обещает явную ошибку. Реализован **bail** «Tron forks are tip-only:
state is served only at the chain tip (/jsonrpc serves state only at TAG latest).
Drop --fork-block-number for a tip fork.». Ключевание — на СЫРОМ значении
`evm_opts.fork_block_number` ДО авто-пина `EvmOpts::get_fork` (`.or(latest)`). Гард —
в общей точке-схождении `TestArgs::run_tests` (до `infer_network_from_fork`), плюс
fail-fast дубликаты в `compile_project`/`compile_and_run_brutalized`/
`CoverageArgs::run` (coverage строит и гоняет тесты сам, обходил `run_tests`-путь —
поймано hotfix'ом). Зеркальный гард в чит-коде `vm.createFork`/`createSelectFork(url,
block)` (`_1`-варианты, `block.is_some()`; `crates/cheatcodes/src/evm/fork.rs`).
Безблочные и at-transaction варианты не задеты; G2-unpin и Stage-1 отказ `forge
script` fork не тронуты. Тесты: оффлайн CLI `tron_historical_fork_block_bails`,
`tron_historical_coverage_fork_block_bails` (bail до сети/tron-solc, снапшот stderr,
покрывают test- и coverage-пути) + юнит `tron_tip_only_fork_guard` (`fork.rs`, матрица
is_tron × block); live-гейт `tron_createselectfork_historical_block_fork_reverts`.
(Полная история hotfix'а зафиксирована в секции «План G», follow-up F+G закрыт.)

**H2 — gas-report energy + bandwidth.** На tron-прогоне (run-level
`config.networks.is_tron()`) `forge test --gas-report` переименовывает колонку gas →
energy (`trace.gas_used` = TVM energy на tron) и добавляет bandwidth-блок (байты).
Оценщик bandwidth реюзает реальные билдеры provider'а (`build_trigger_raw`/
`build_create_raw`) с плейсхолдерными TAPOS/timestamp + 65-байтовой dummy-подписью →
`encode_to_vec().len() + 64` (java-tron `BandwidthProcessor.consume`:
`serialized_size(подписанный protobuf-tx, ret очищен) + MAX_RESULT_SIZE_IN_TX(64)`).
Точно для нашего broadcast-пути (билдер НЕ ставит `ref_block_num`; чужие кошельки
могут отличаться на пару байт). EVM-вывод gas-report — БАЙТ-В-БАЙТ без изменений
(гейт is_tron; JSON-поля — `Option` + `#[serde(default, skip_serializing_if)]`).
Deployment energy = метеринг create-фрейма (Counter в `setUp` — depth>1 create —
даёт **101191**; I7-фикс пишет `contract_info.gas` до depth-guard'а, раньше guard ронял
его в 0), deployment bandwidth точен из initcode. Тесты: оффлайн-юниты оценщика против committed-фикстур —
mainnet trigger → **345**, Nile create → **853** (точные значения, без допусков);
CLI `tron_gas_report_energy_and_bandwidth` (гейт: tron-solc в кэше) —
детерминированные bandwidth-константы `increment()` → 280, `setNumber(uint256)` → 314;
существующие EVM gas-report снапшоты (`cmd.rs`) не тронуты. `forge snapshot` (парсит
`(gas: N)` регэкспом, не таблицу) НЕ трогали.

**H3 — TRONSCAN verify: провайдер + standalone `forge verify-contract`.** Новый
keyless, СИНХРОННЫЙ (нет GUID-поллинга) провайдер `crates/verify/src/tronscan/mod.rs`
(`VerificationProviderType::Tronscan`). `VerifyArgs::run` ветвится на
`config.networks.is_tron()` ДО alloy chain-резолюции (зеркало `forge create`), форсит
TronScan-провайдера, пропускает aux-Sourcify в `collect_runs`. Submit —
`multipart/form-data` POST `/api/solidity/contract/verify`: `contractAddress`=base58
(`to_base58`), флаттеный исходник (`foundry_common::flatten`, без vanilla-solc dry-run
и без ipfs-ассерта — tron-байткод на ванильном solc не идёт), `constructorParams`=hex
без `0x`, `compiler`=пин `tron_v<longVersion>`. `evmVersion` нормализуется против
компилятора (`normalize_version_solc`): слишком новый дефолт (`osaka`) шлётся как
реальный fork tron-solc (prague для 0.8.27, cancun для 0.8.25/0.8.26) — TRONSCAN
отвергает версию, которой компилятор не пользовался. Маппинг `data.status`: 2006=успех,
2001=already verified (не ошибка), 2007/2008=fail; `code!=200` → hard error с `errmsg`;
retry-обёртка как в etherscan. `check`/`--watch` — re-query
`/api/solidity/contract/info` (`data.status==2`). Host-routing по chain id из
конфига/RPC: mainnet 728126428 → `apilist.tronscanapi.com`, Nile 3448148188 →
`nileapi.tronscan.org`, оверрайд `--verifier-url`, неопределим → понятная ошибка.
Пины long-version — `crates/tron/solc/src/pins.rs` (commit/longVersion per версия из
того же list.json, экспорт `tronscan_compiler_string(&Version)`). Cargo: reqwest
`multipart` + `foundry-tron-primitives`/`foundry-tron-solc` в crates/verify. Тесты:
оффлайн-юниты `assemble_fields` (все поля/имена/формы: base58 `contractAddress`, hex
без `0x`, optimizer/runs/viaIR/license, `evmVersion`-нормализация, конструктор-селектор,
`0x`-strip); live-гейт `TRON_LIVE` `live_tronscan_info_compiler_string` — re-query
`/info` known-verified mainnet `TMv7hAfswe2EvXG4nUeNFGEgNWE8Joedtu`
(compiler `tron_v0.8.25+commit.77bd169f`). **Acceptance-гейт (live, тратит TRX,
double-gate `TRON_LIVE=1` + `TRON_VERIFY_E2E=1`, тест
`live_tron_verify_contract_e2e_on_nile`) ПРОЙДЕН:** свежий Counter задеплоен и
верифицирован на Nile через `forge verify-contract`, compiler
`tron_v0.8.27+commit.19164bed`, достигнут `/info` status 2 — пин 0.8.27 подтверждён
первой реальной верификацией (риск R3 снят, формат compiler-строки для 0.8.x
корректен).

**H4 — embedded `forge create --verify` + address-парс.** `forge create --verify` на
tron разблокирован (`create.rs`): после broadcast контракт маршрутизируется в TronScan
через `VerifyArgs::run`, который повторно входит в tron-ветку (`network = "tron"`) и
реюзает ту же keyless-сабмиту, что standalone `forge verify-contract`. Pre-broadcast
preflight (пин compiler-строки, routable TronScan-хост, флаттен) падает ДО траты TRX
(зеркало `run_generic`). `--unlocked`/`--browser` остаются bail. Адресный аргумент
`forge verify-contract` принимает `T…`/`41…` (value-parser: сначала EVM-парс — non-tron
поведение не тронуто, — для non-`0x` фолбэк на tron-кодек). **`forge script --verify` —
оставлен явным bail с точной причиной (follow-up DEFER):** шов асимметричен
(etherscan-центричный `verify_contracts`/`VerifyBundle`, гейт на etherscan-ключ,
tron-ветка `run` не зовёт `broadcasted.verify()`) — разблокировка не проверяема live без
траты TRX в рамках H4. Тесты: юнит на парс адреса; CLI
`tron_create_verify_preflight_no_longer_bails` (dry-run без `--broadcast`: preflight
проходит оффлайн, команда доходит до dry-run вместо снятого bail); non-tron `--verify`
не задет.

**H5 — документация + hygiene + version-stamp.** Новый self-contained английский
`docs/tron/USER_GUIDE.md` (410 строк): quickstart (`network="tron"`, `[rpc_endpoints]`,
авторезолв tron-solc, sandbox-walkthrough); матрица покрытия команд (works / explicit
error / out-of-scope — включая gas-report, verify-contract, fork-guard); справочник
`[tron]`-конфига + пер-командные оверрайды; форматы адресов; VM/energy-дельты
(CREATE-схема, depth 64, ISCONTRACT, bandwidth «для foundry-broadcast tx»); fork-режим
(tip-only, mainnet-only `/jsonrpc`, read-only, явная ошибка на исторический блок);
верификация TRONSCAN (keyless, flattened, хосты, license-коды); env-гейты live-тестов;
foundryup из форка (`FOUNDRYUP_REPO=elsvv/foundry-tron`). Root `README.md` — секция
«Tron support» со ссылкой (стоковый upstream-текст не перелопачен).
`sandbox/tron-counter/README.md` — убран мёртвый путь `…/foundry/target/…` и устаревший
OpcodeNotFound-репро, заменён актуальным build/test через авторезолв. Version-stamp:
`forge --version` (+cast/anvil/chisel) несёт маркер `tron` внутри существующей скобки
short-версии и на строке `Version:` long-версии (`crates/common/build.rs`); SemVer-строка
НЕ тронута — `strip_semver_metadata` (`foundryVersionCmp`/`foundryVersionAtLeast`) парсит
чисто, снапшоты версии `<version> (<...>)` зелёные.

**H6 — CI tron-live workflow.** Новый самодостаточный `.github/workflows/tron-live.yml`:
weekly cron (`0 6 * * 1`, Пн 06:00 UTC) + `workflow_dispatch`. Job `fork-reads` —
keyless + read-only, `TRON_LIVE=1`, БЕЗ секретов:
`cargo nextest run --locked -p forge -E 'test(tron_mainnet_fork_reads_usdt)'` (mainnet
`/jsonrpc` fork USDT; tron-solc авторезолвится download'ом на пустом CI-кэше). Второй job
`golden` — manual-only (`if: github.event_name == 'workflow_dispatch'`), секреты
`TRON_PRIVATE_KEY`/`TRON_PRO_API_KEY`:
`cargo nextest run --locked -p foundry-tron-provider -E 'test(live_)'` (тратит Nile TRX +
деплоит — комментарий про faucet-пополнение; verify E2E остаётся скипнут, `TRON_VERIFY_E2E`
не задан — это one-time acceptance-гейт, не рекуррентный golden). Nile `/wallet`-пробы в
weekly cron НЕ включены (WAF-риск) — только в manual golden с `TRON_PRO_API_KEY`. НЕ зависит
от reusable `tempoxyz/*`-workflows, НЕ трогает существующую матрицу (`test.yml`/
`matrices.py` — оффлайн-tron-тесты уже в ней, фильтр только `!ext_integration`; live
самоскипаются без `TRON_LIVE`). `runs-on: ubuntu-latest` (GitHub-hosted, не depot —
fork-safe); экшены пиненые теми же SHA, что и в репо; отдельный workflow не гейтит PR
(non-blocking по построению — не на push/pull_request). **Замечание:** CI форка НИ РАЗУ не
бегал (`total_count=0` до этого) — первый полный прогон матрицы = отдельное событие после
пуша; латентные красноты полной матрицы (clippy/fmt/deny/typos) чинятся follow-up'ом, H6
отвечает только за корректность нового workflow-файла.

**Живые подтверждения плана H:** verify E2E на Nile (H3 acceptance, свежий Counter,
`tron_v0.8.27+commit.19164bed`, `/info` status 2); read-only re-query `/info` mainnet
`TMv7hAfswe2EvXG4nUeNFGEgNWE8Joedtu` (compiler `tron_v0.8.25+commit.77bd169f`). Всё
оффлайновое покрыто юнит/CLI-тестами; live-каналы гейтятся `TRON_LIVE`/`TRON_VERIFY_E2E`.

**DEFER (вне скоупа Этапа 3, зафиксировано):**

- Вынос tron-крейтов в отдельные репо — tron-логика энтэнглена (`evm/core` inline-модули
  `evm/tron/*`, inline `tron.rs` в cast/config/script/cli); вынос при реальном OSS-релизе
  за трейт-границей.
- Публикация бинарных релизов / Docker / ребрендинг installer'а — после первого зелёного
  CI и живой верификации; foundryup уже параметризован `FOUNDRYUP_REPO`, `release.yml`
  fork-safe (`${{ github.repository }}`), Docker/installer захардкожены на foundry-rs
  (документировать в USER_GUIDE, не менять).
- `forge script --verify` через TronScan — шов асимметричен (H4); follow-up: развести
  `verify_contracts`/`VerifyBundle` на TronScan-провайдера + live-E2E (Nile). Standalone
  `forge verify-contract` и `forge create --verify` уже покрывают потребность.
- Standard-json / multi-file верификация TRONSCAN (поддержка не доказана) — только flattened.
- Скраб русских внутренних `docs/tron/*` — USER_GUIDE их суперсидит для пользователей.
- Первый зелёный прогон CI форка под полной матрицей (0 runs до сих пор) — событие после
  пуша, латентные красноты чинятся follow-up'ом.

## План I — Этап 4 «fidelity» (ветка `tron-stage4`)

Закрытие находок fidelity-аудита 2026-07-23. Всё, что делает mainnet-оценки энергии/
стоимости доверяемыми для «горячих» контрактов, а деплой/send-пути — не отдающими ноде
отвергаемые транзакции. Порядок исполнения I1→I9 (I5 — фундамент для I6/I7).

1. **I1 — precompile-кламп.** Карта precompile'ов tron-EVM строится ТОЛЬКО из
   `tron_precompiles()` (без эфирной базы спека конфига). Эфирные BLS12-381 (0x0b–0x11) и
   P256Verify (0x100) больше не протекают ни при каком `evm_version`. Вызов отсутствующего
   адреса = пустой аккаунт (java-tron).
2. **I2 — value-only вызов контракта.** `cast send <контракт> --value N` без сигнатуры и
   broadcast-петля script'а строят `TriggerSmartContract` (легальный payable
   fallback/receive), а не `TransferContract` (нода отвергает его на контракт-адрес). Новый
   `TronProvider::is_contract` (`/wallet/getcontract`).
3. **I3 — tron-solc 0.8.28 + динамический резолв.** Пины расширены до 0.8.23–0.8.28,
   `default_version()` = **0.8.28**. Версия вне пинов при `!offline` резолвится из
   `tronprotocol.github.io/solc-bin/{list_key}/list.json` с проверкой опубликованной sha256;
   `offline` по-прежнему детерминированно падает `NoPin`.
4. **I4 — идентичность сети.** Локальный дефолт `block.chainid` = mainnet **728126428** (был
   31337 — ломал EIP-712/permit); `block.basefee` = `getEnergyFee` (100 sun) и переопределяем
   `vm.fee`; `vm.prevrandao`/`vm.txGasPrice` на tron один раз варнят (TVM жёстко 0 — no-op, не
   ошибка).
5. **I5 — chain-параметры.** `TronChainParams` + `get_chain_parameters()`
   (`/wallet/getchainparameters`). Fork best-effort warn при устаревшей цене энергии / Osaka.
   Оффлайн-дефолты = live-снапшот 2026-07-23 (см. ниже).
6. **I6 — петля оценки.** `ConstantResult.energy_penalty`, расширенный `TxInfo`
   (energy_penalty_total/usage/origin/net), `estimate_energy`, `get_contract_energy_factor`,
   `suggest_fee_limit_sun` (буфер 20%, кламп 15 000 TRX), `cast estimate` на tron (text+json),
   валидация `fee_limit > getMaxFeeLimit` перед broadcast.
7. **I7 — TIP-491 в симуляции.** Penalty считается ПОСТ-ФАКТУМ из trace-арены (собственная
   энергия узла × factor(address)/10000), факторы тянутся с fork-ноды
   (`get_contract_energy_factor`) и кэшируются на прогон. `forge test --gas-report` на форке
   рисует колонку **Penalty Avg** + ячейку **Deployment Penalty** (JSON `energy_penalty`,
   `skip_serializing_if`); non-fork — пустая карта, репорт base-only и байт-в-байт неизменен.
   Knob `[tron] dynamic_energy` (default true). Отдельный фикс: **Deployment Energy для
   вложенных create** (tron-gated запись `contract_info.gas` до depth-guard'а — EVM
   `gas_report_size_for_nested_create`/#9300 остаётся 0, байт-в-байт). Ограничение:
   `gasleft()` внутри горячего фрейма — base-only (penalty не в интерпретаторе).
8. **I8 — base58 в трейсах.** Два шва декодера: незалейбленный контракт идентифицируется
   base58-формой (`vm.label`/known-контракты приоритетны); address-значения в
   args/returns/logs (в т.ч. вложенные) — base58. `TraceWriter` — внешний крейт без хука;
   `foundry-common-fmt` получил `format_token_with_address`, декодер держит форматтер как
   `fn(Address)->String` из вызова (traces без tron-dep). Off-tron — байт-в-байт.
9. **I9 — release prep.** Тулчейн-версия штампа `--version` → **0.2.0** (`(tron 0.2.0; …)`,
   `Version: … (tron fork 0.2.0)`); STATUS/USER_GUIDE обновлены; feature-списки Makefile ↔
   `foundry-tron-build.yml` сверены.

**Снапшот-константы (live-проба 2026-07-23, mainnet `api.trongrid.io`) — оффлайн-дефолты
`TronChainParams`:** `getEnergyFee` 100 sun, `getMaxFeeLimit` 15 000 000 000 sun (15 000 TRX),
`getTransactionFee` 1000 sun/байт, `getMemoFee` 1 000 000 sun, `getDynamicEnergyThreshold`
5 000 000 000, `getDynamicEnergyIncreaseFactor` 2000 (+20%), `getDynamicEnergyMaxFactor` 34000
(3.4 → до 4.4× total), `getAllowTvmOsaka` 0. USDT стоит на максимуме `energy_factor` = 34000.
**tron-solc 0.8.28** — `0.8.28+commit.9c4253d2`, sha256 (macosx/linux/windows) запинены в
`pins.rs` (сняты raw curl'ом 2026-07-23).

**Live-гейты (не в оффлайн-прогоне):** `TRON_LIVE=1` — I5 staleness-датчик (mainnet
`getchainparameters` == дефолты, печатает диф), I6 USDT-оценка (`energy_penalty>0`,
`energy_used≈base×4.4`), I7 fork gas-report penalty (mainnet fork, `Penalty Avg` рисуется из
node-fetch'а); I2 live-деплой `PayableSink` + value-only send — **Nile** (spend). Nile
активирует апгрейды РАНЬШЕ mainnet — параметры двух сетей могут расходиться.

**DEFER (вне скоупа Этапа 4, зафиксировано):** Osaka-вариант precompile/энергомодели (ждём
активации на Nile; сторожок — warn из I5); anvil-tron / chisel-tron как настоящие
TVM-окружения (бинарники — vanilla-EVM); in-loop метеринг penalty (`gasleft()` в горячих
контрактах); TRC-10 семантика (0xD0–0xD3 — стабы); stake/gov-опкоды 0xd5–0xdf;
multisig/permission_id/memo в билдере; глубина вызовов 64 (revm `CALL_STACK_LIMIT`); учёт
1.1 TRX activation-burn; читкод `vm.tronSetEnergyFactor` для локального моделирования penalty.

## Ключевые находки (не потерять)

00. **Golden поймал реальную дельту: MLOAD/MSTORE/MSTORE8 = SPECIAL_TIER (1), а не VERY_LOW (3).** Первый прогон golden дал расхождение view `number()` local 422 vs node 414 (Δ8) и write local 20440 vs node 20438 (Δ2). Корень — java-tron `OperationRegistry.adjustMemOperations` перерегистрирует MLOAD/MSTORE/MSTORE8 на `getMloadCost2`/`getMStoreCost2`/`getMStore8Cost2` = `SPECIAL_TIER(1) + calcMemEnergy`, когда активен `allowHigherLimitForMaxCpuTimeOfOneTx` (chain param `getAllowHigherLimitForMaxCpuTimeOfOneTx=1` на Nile, проба 2026-07-12). revm-FRONTIER несёт для них 3 в статической таблице (память — динамически внутри инструкции), поэтому фикс = `insert_gas(MLOAD/MSTORE/MSTORE8, 1)`. Δ8 = 2 MSTORE + 2 MLOAD × 2; Δ2 = 1 MSTORE × 2 — оба обнулились до exact-match. MCOPY НЕ трогается (`getMCopyCost` держит VERY_LOW=3, совпадает с revm). Зафиксировано в `energy.rs` (`TRON_MEMORY_OP_ENERGY`) + два юнит-теста. **Это доказывает ценность golden с exact-match без допусков — «почти совпадает» скрыло бы дельту.**

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
10. **Precompile `0x03` на Tron — это НЕ ripemd160, а `sha256(sha256(data)[0..20])`** (усечение `[0..20]` — у ВНУТРЕННЕГО хеша; выход — полные 32 байта), 600+120/word (`PrecompiledContracts.java:564-577`). Настоящий ripemd160 переехал на `0x020003`, blake2f — на `0x020009` (активация под `allowTvmCompatibleEvm`; на mainnet флаг выкл., но адреса вне досягаемости обычного кода — ставим безвредно). `0x09`/`0x0a` замещены на BatchValidateSign (TIP-43, energy `((len/32-5)/6)*1500`) / ValidateMultiSign (TIP-60) — перекрывают blake2f@0x09 и KZG@0x0a (намеренно, покрыто контраст-тестом). Live-подтверждено на Nile: `0x01` ECRecover возвращает **21-байтовый** (0x41-префикс) адрес в word; `0x03("abc")` = двойной sha256 (`live_precompile_probe_on_nile`).
11. **Канал доступа к исходникам java-tron: jsdelivr.** `raw.githubusercontent.com` с этой машины блокируется; `https://cdn.jsdelivr.net/gh/tronprotocol/java-tron@develop/<path>` работает (`curl -4`). Все доменные факты плана E сверялись через него (`EnergyCost.java`, `OperationRegistry.java`, `PrecompiledContracts.java`, `Program.java`, `ProgramResult.java`).
13. **java-tron `/jsonrpc` отдаёт account/storage/code ТОЛЬКО на TAG `latest`** — на конкретный номер блока (QUANTITY) возвращает `-32602 "QUANTITY not supported, just support TAG as latest"`. Это НЕ то же, что nonce-стаб: `getBalance/getStorageAt/getCode` действительно живые, но fork-backend пинит их на номер fork-блока → падение даже после nonce-shim'а. Отсюда второй слой плана G: для tron НЕ пиним state-блок в `MultiFork::create_fork` (`SharedBackend::new(..., None)` → fork-db шлёт `latest`), fork становится tip-only. Разведка плана G это пропустила (пробовала методы с `latest`, а не с номером блока) — поймано только live-прогоном USDT-теста. Block-env при этом остаётся пиненым на номер fork-блока (`get_block_by_number(number)` java-tron поддерживает — им же BLOCKHASH и питается).
12. **Live-нода: `https://api.nileex.io`, НЕ `nile.trongrid.io`.** С этой машины `nile.trongrid.io` недоступен (DNS/сеть); `api.nileex.io` работает для `/wallet/*`. ВАЖНО: на nileex `/jsonrpc` НЕ смонтирован (только `/wallet/*`) — поэтому `get_chain_id`/fork-режим (план G) должны учитывать, что eth_* JSON-RPC живёт на другом хосте/пути. Часть старых live-тестов провайдера всё ещё указывает на `nile.trongrid.io` (могут флапать TLS-хэндшейком) — новые (precompile/create2/golden) уже на `api.nileex.io`.

## Следующие шаги (после Этапа 3 / плана H)

- **Этап 2** (планы E/F/G) — ✅ ГОТОВ: energy/precompiles/CREATE2 (E), резолвер tron-solc (F), read-only fork через `/jsonrpc` (G). См. секции «План E/F/G» выше.
- **Этап 3 / план H** — ✅ ГОТОВ: gas-report energy+bandwidth, fork-guard на исторический блок, TRONSCAN verify (`forge verify-contract` + embedded `forge create --verify`), USER_GUIDE/README/version-stamp, CI-workflow `tron-live`. См. секцию «План H» выше. Live acceptance-гейт verify E2E пройден на Nile.
- **Дистрибуция тулчейна** — ✅ ОТГРУЖЕНА (влито в `master`, PR #6): multi-platform CI-сборка `foundry-tron-build.yml` (darwin arm64/amd64, linux arm64/amd64, win32 amd64; feature-surface зеркалит `release.yml`) публикует четыре `*-tron` бинарника (forge-tron, cast-tron, anvil-tron, chisel-tron) в rolling-prerelease с фиксированным тегом **`foundry-tron-latest`** (5 арх-архивов + `.sha256`-сайдкары). Keyless one-liner-инсталлятор `install-foundry-tron.sh` (+ `install-foundry-tron.ps1` для Windows) детектит OS/arch, качает и сверяет sha256, распаковывает в `~/.foundry-tron/bin`, PATH правит только по opt-in `--modify-path`, с фолбэком на `gh release download` когда asset-хост недоступен. Установка:
  ```bash
  curl -fsSL https://raw.githubusercontent.com/elsvv/foundry-tron/master/install-foundry-tron.sh | bash
  ```
  Плюс пины tron-solc `0.8.23`/`0.8.24` (sha256 + longVersion, три платформы) для авто-резолва под этими pragma.
- **Пилот tron-1inch** — В РАБОТЕ: тулчейн `*-tron` обкатывается на реальном TronBox-проекте `stonfi/tron-1inch` (1inch escrow/custody для Tron, ветка `feat/init-foundry-tron`) — миграция на foundry-tron как первый внешний потребитель дистрибуции.
- **Осталось (DEFER, к OSS-релизу):** вынос tron-крейтов за трейт-границу, Docker-образ, `forge script --verify` через TronScan. Детали и обоснования — в блоке «DEFER» плана H. (Первый зелёный прогон CI-сборки под полной матрицей — ✅ ВЫПОЛНЕН через `foundry-tron-build`; публикация бинарных релизов — ✅ ВЫПОЛНЕНА, см. «Дистрибуция тулчейна».)

## Окружение (важно для любой машины)

- **Тулчейн:** на текущей машине (`andrey`) — свежий rustup (stable 1.97.0 по умолчанию, nightly с rustfmt). Обычный `cargo` работает; перед командами достаточно `export PATH="$HOME/.cargo/bin:$PATH"`. Форматирование — `cargo +nightly fmt` (repo `rustfmt.toml`).
  Зависимости требуют rustc ≥1.91. Fallback для машин, где системный/Homebrew rustc старее и перекрывает rustup (как на исходной машине с Homebrew 1.88): все cargo-команды через явный путь к stable-тулчейну:
  ```bash
  TC=$(dirname "$(rustup which --toolchain stable cargo)"); PATH="$TC:$PATH" cargo <...>
  ```
- **tron-solc:** нативный бинарник `0.8.27` — `/Users/andrey/.foundry-tron/solc/tron-solc-0.8.27` (universal macOS, sha256 `9e369b44…c7ce17aa`). После плана F резолвится **авто** через `foundry-tron-solc` при `network=tron` (кэш-хит без сети; при пустом кэше — скачивание с пин-верификацией, отключается `offline=true`); `sandbox/tron-counter/foundry.toml` `solc` НЕ задаёт. Явный `solc = "/abs/path"` по-прежнему оверрайдит резолвер (`SolcReq::Local` не разворачивает `~`, поэтому путь абсолютный).
- **Live-тесты:** `TRON_LIVE=1` + `TRON_PRIVATE_KEY` (файл `.env.tron-dev` в корне репо, в git НЕ входит — перенести вручную или сгенерировать новый ключ и пополнить через кран https://nileex.io/join/getJoinPage). Текущий тестовый адрес: `TX7izXWcmofRYonzdcThrS78jifMtVWCuf` (~1997 TRX на Nile).
- Тесты tron-крейтов: `cargo test -p foundry-tron-primitives -p foundry-tron-provider`. CI-линт: `cargo clippy --all-targets` с `-Dwarnings` — tron-крейты чистые.

## Процесс работы (утверждено пользователем)

- Планы исполняются dynamic-workflow: исполнители Opus → Opus-верификатор после каждой задачи (fix-цикл до 3) → Fable-ревью после каждых двух задач и в конце (тоже с fix-циклом). Шаблон скрипта — `docs/tron/workflow-template.js` (+ README рядом).
- Требования к тестам: только реальные векторы/fixtures (сеть, официальные .proto, java-tron), никакой тавтологии, запрещено ослаблять ассерты и ставить `#[ignore]`; live-тесты гейтятся env-переменной с явным `eprintln("skipped…")`.
- Верификаторы обязаны независимо перепроверять доменные факты (повторные curl, пересчёт векторов, сверка с первоисточниками) — этот паттерн уже поймал 2 неверных вектора в планах и находку №1.
