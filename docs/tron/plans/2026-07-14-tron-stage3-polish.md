# План H — Этап 3 «полировка»: gas-report, fork-guard, TRONSCAN-верификация, доки, CI

Дата: 2026-07-14. Статус: утверждён к исполнению (dynamic workflow, ветка `tron-stage3` от `tron-dev`).

Закрывает: хвосты Этапа 2 (спека §4.4: gas-report energy+bandwidth; follow-up финального ревью F+G:
молчаливый исторический fork-блок) и Этап 3 спеки §6/§4.8 (верификация TRONSCAN → документация →
CI/golden). «Бинарные релизы» — только version-stamp; «вынос tron-крейтов» — DEFER (см. «Вне скоупа»).

Скаут-отчёты этой сессии (машиноспецифичные пути, живут до конца сессии):
`/private/tmp/claude-501/-Users-vaceslaveliseev--dev-foundry-tron/41b5efea-d175-460c-b93f-3371d9da9d74/scratchpad/scout-stage3/{gas-report.md,tronscan-verify.md,polish.md}`.
Все load-bearing факты из них продублированы ниже — план самодостаточен.

---

## Проверенные факты (основания; верификаторы обязаны перепроверять первоисточники)

### F-1. Energy уже в gas-report — это существующая колонка gas

На tron-пути `trace.gas_used` == TVM energy (golden плана E: local == нода exact-match, view 414 /
write 20438). Интринсик занулён (`energy.rs:181-183` — интринсик это bandwidth, не energy).
Значит колонка «gas» уже несёт energy — нужен только relabel + вторая колонка bandwidth.

### F-2. Формула bandwidth (java-tron, подтверждена live с точностью до байта)

`BandwidthProcessor.consume` (java-tron @develop, `chainbase/.../db/BandwidthProcessor.java`):
```
bytesSize = trx.toBuilder().clearRet().build().getSerializedSize();
bytesSize += MAX_RESULT_SIZE_IN_TX;   // 64, per contract; Constant.java
```
⇒ **bandwidth (байты) = serialized_size(подписанный protobuf-Transaction, ret очищен) + 64**.
Для свежесобранной tx `ret` пуст → `clearRet()` no-op. Подпись фиксированно 65 байт
(`PER_SIGN_LENGTH=65`; наш `sign.rs:28-33`). Порядок списания: staked net → free net → сжигание
`bytes × getTransactionFee()` (mainnet 1000 SUN/байт). Репорт показывает **байты**, не SUN.

**Live-валидация (2026-07-14):** фикстура `crates/tron/primitives/testdata/mainnet_trigger_tx.json`
(реальный `transfer(address,uint256)`, calldata 68 байт): raw_data 211 байт → signed tx 281 байт →
+64 = **345**. Receipt ноды по txID `05207fac…23104d`: `net_fee = 345000` SUN = 345 байт × 1000 —
совпадение до байта. Create-фикстура `nile_create_tx.json`: raw_data 719 → **853**.

Оценка детерминирована оффлайн: переменные только calldata/initcode (`D`/`L`), имя контракта
(create), `fee_limit` (varint из конфига), `call_value`. TAPOS — фиксированные 2+8 байт,
expiration/timestamp — 6-байтовые varint для любой реалистичной эпохи. `Any` type_url фиксирован
(49 байт для TriggerSmartContract). Наш билдер НЕ ставит `ref_block_num` (tag 3) — оценка точна
для нашего broadcast-пути; чужие кошельки могут отличаться на пару байт (документировать).

### F-3. Швы gas-report

- Сбор/рендер: `crates/forge/src/gas_report.rs` (весь файл, ~282 строки). Деплой:
  `contract_info.gas = trace.gas_used`, `size = trace.data.len()` (:96-97,108-109; для create
  `trace.data` = initcode). Вызовы: `gas_info.frames.push(trace.gas_used)` (:121). Только
  top-level фреймы (:101-103). `finalize()` :128-143.
- Единственная точка сборки, где доступен `config`: `crates/forge/src/cmd/test/mod.rs:2762-2768`
  (`GasReport::new(...)`); там же `config.networks.is_tron()`
  (`crates/evm/networks/src/lib.rs:229`) и `config.tron` (`crates/config/src/tron.rs:25-58`).
- Анализ: test/mod.rs:2940-2956 (включая fuzz `gas_report_traces`); финализация :3087-3090.
- JSON: `format_json_output` (:167-201) — ad-hoc `json!`-объекты. Новые поля — ТОЛЬКО
  `Option<..>` + `#[serde(default, skip_serializing_if = "Option::is_none")]`, EVM-снапшоты
  байт-в-байт (паттерн блока `tron{}` в broadcast-артефакте плана D).
- Таблица: `format_table_output` (:203-260, comfy_table), литеральные заголовки :213-232.
- `forge snapshot` НЕ ТРОГАТЬ: он парсит `(gas: N)` из текста результата регэкспом
  (`snapshot.rs:25`), таблицу не читает; на tron это уже energy.
- CLI-шаблоны тестов: `gas_report_all_contracts` и соседи (`crates/forge/tests/cli/cmd.rs:2039+`,
  таблица + `--json`-снапшоты). Tron CLI-тесты: `crates/forge/tests/cli/tron.rs`.
- Per-test network override (`is_multi_pass`, test/mod.rs:2785) с `--gas-report` — комбинация не
  поддерживается; гейтить bandwidth-ветку на run-level `config.networks.is_tron()`.

### F-4. Fork-guard (follow-up F+G)

Проблема: `--fork-block-number <исторический>` на tron проходит молча (block env пиннится, state
tip-only) — спека §4.4 обещает явную ошибку. **Решение: BAIL** (утверждено: рекомендация ревью
F+G + скаута) с текстом, объясняющим tip-only и что убрать.

Ловушка: наивный `fork_block_number.is_some()` в `create_fork` НЕ РАБОТАЕТ — `get_fork`
(`crates/evm/core/src/opts.rs:367-368`) авто-пинит `.or(fork_block_number)` на latest, к моменту
`create_fork` там всегда `Some`. Ключевать на СЫРОМ пользовательском значении до авто-пина.

Две точки входа, обе гардить:
1. CLI `--fork-block-number`: pre-flight в forge test cmd после загрузки config/evm_opts (рядом с
   `infer_network_from_fork`, test/mod.rs) — там Result-контекст, можно bail.
2. Чит-код `vm.createFork/createSelectFork(url, blockNumber)`: явный блок приходит как
   `Some(blockNumber)` в `_1`-вариантах (`crates/cheatcodes/src/evm/fork.rs`) — гардить `Some`;
   безблочный вариант передаёт `None`, ложных срабатываний нет.
Unpin-логику G2 (multi.rs:617) не трогать. `forge script` fork-отказ Stage-1 не трогать.

### F-5. TRONSCAN verify API (реверс: frontend + across-protocol + mapprotocol + live-пробы 2026-07-14)

Официальных доков НЕТ (`tronscan-api` deprecated/private). Факты корроборированы тремя
независимыми источниками + live `/info` уже верифицированного контракта:

- **Endpoint**: `POST /api/solidity/contract/verify`, `multipart/form-data`, **KEYLESS**,
  **СИНХРОННЫЙ** (нет GUID-поллинга). Хосты: mainnet (728126428) `https://apilist.tronscanapi.com`
  (алиас `apilist.tronscan.org`); Nile (3448148188) `https://nileapi.tronscan.org`
  (НЕ `nile.tronscan.org` — это фронтенд, отдаёт HTML).
- **Поля формы**: `contractAddress` (base58 `T…`), `contractName`, `compiler`,
  `optimizer` ("1"/"0"), `runs`, `license` (числовой код, таблица Etherscan: 3=MIT, 12=Apache-2.0,
  14=BUSL-1.1…), `evmVersion` (opt), `viaIR` (opt, "1"/"0"),
  `constructorParams` (hex БЕЗ `0x`; именно `constructorParams` — фронтенд+mapprotocol;
  «constructorArguments» в across README — описка), `files` (флаттеный .sol, file upload).
- **Ответ**: `{code, errmsg, data:{status, message}}`; `code==200` + `data.status`: 2006=успех,
  2001=уже верифицирован, 2007/2008=fail (wrong parameters). `code!=200` → hard error c `errmsg`.
- **Compiler string**: `tron_v<solc long version>`. **Live-подтверждено**: контракт
  `TMv7hAfswe2EvXG4nUeNFGEgNWE8Joedtu` (across AcrossEventEmitter, mainnet) хранит
  `compiler="tron_v0.8.25+commit.77bd169f"`, evm=cancun, via_ir=1, runs=800, license="14",
  status=2 (=verified). Коммиты пиненых версий из `tronprotocol.github.io/solc-bin/*/list.json`
  `builds[].longVersion`: 0.8.25=`77bd169f` (live-подтверждён), 0.8.26=`733b4d28`,
  0.8.27=`19164bed` (выведены из авторитетного list.json, live-сабмитом НЕ проверены — первая
  реальная верификация на Nile = acceptance-гейт). Легаси-формат `tron-0.4.25_Odyssey_v3.2.3`
  (USDT) — для 0.8.x НЕ использовать.
- **Confirm**: `POST /api/solidity/contract/info` `{"contractAddress":"T…"}` → сохранённые
  settings, `data.status==2` = verified. `/api/contract?contract=` кэшируется и лагает — не юзать.
- **Источник**: флаттеный одиночный файл (across: «single flattened file», прагму нормализуют
  sed'ом). Multi-file не доказан, standard-json НЕ известен как поддержанный — только flattened.

### F-6. Швы crates/verify

- Трейт `VerificationProvider`: `crates/verify/src/provider.rs:92-135`
  (`preflight_verify_check`, `submit` → `Result<Option<VerifyCheckArgs>>`, `verify`, `check`).
  Синхронность TRONSCAN ⇒ `submit` возвращает терминальный результат; `check` — re-query `/info`.
- Enum+регистрация: `VerificationProviderType` :175-184 (добавить `Tronscan`), `FromStr` :137-150,
  `Display` :152-173, фабрика `client(...)` :186-267.
- Run-флоу: `VerifyArgs::run` `crates/verify/src/verify.rs:554-677`; `collect_runs` :683-727
  (на tron пропустить aux-Sourcify). **Диспетч**: ранний бранч на `config.networks.is_tron()` ДО
  alloy chain-резолюции — зеркало `create.rs:154`.
- `VerifyArgs` уже несёт всё нужное: `address` (20-байтовый; base58 через
  `foundry_tron_primitives::address::to_base58`, `address.rs:30`), `contract`,
  `constructor_args`/`guess_constructor_args`, `compiler_version` (голый semver — коммита НЕТ),
  `num_of_optimizations`, `license_type` (уже парсит числовые коды/SPDX, verify.rs:389-402),
  `evm_version`, `via_ir`, `flatten`/`force`, `VerifierArgs{verifier, verifier_url}`.
- Флаттенинг: реюз `foundry_common::flatten` (как `etherscan/flatten.rs:36`); vanilla-solc
  dry-run (`check_flattened`, flatten.rs:72-105) на tron ПРОПУСТИТЬ (vanilla solc не тот
  компилятор, tron-байткод на ванильном revm не идёт); ipfs-ассерт (flatten.rs:30-34) не применять.
- Гэп long-version: tron-solc резолвер не exec'ает `--version` (план F), коммит не захвачен.
  **Решение — пины**: добавить commit/longVersion в `crates/tron/solc/src/pins.rs` (источник тот
  же list.json), экспорт `tronscan_compiler_string(&Version) -> Option<String>`.
- Cargo: `crates/verify` — добавить feature `multipart` к reqwest + dep `foundry-tron-primitives`
  (+ `foundry-tron-solc` для compiler-string).
- Embedded: `create.rs:189-197` сейчас bail'ит `--verify … not supported on tron yet`.
- Routing chain id: на tron-пути alloy `get_chain` ненадёжен; хост выбирать по chain id из
  конфига/RPC (mainnet 728126428 / nile 3448148188), при неопределимости — требовать
  `--verifier-url` с понятной ошибкой. Это главный routing-вопрос — решить в H3 явно.

### F-7. Доки/CI/релизы (инвентарь)

- Пользовательских доков НЕТ: `grep -rlni tron README.md docs/dev/` пуст; root README — стоковый
  upstream. Внутренние `docs/tron/*` — русские, машиноспецифичные, для OSS не годятся.
- `[tron]`-конфиг (сверено с `crates/config/src/tron.rs`): `fee_limit=1_000_000_000` SUN,
  `origin_energy_limit=10_000_000`, `user_fee_percentage=100`, `expiration=60` с; оверрайды
  `--tron.fee-limit`/`--tron.expiration`.
- CI форка НИ РАЗУ не запускался: `gh api repos/elsvv/foundry-tron/actions/runs` → total_count=0.
  Workflows унаследованы (state: active), матрица `.github/scripts/matrices.py` фильтрует только
  `ext_integration` — tron-крейты УЖЕ в workspace-прогоне (feature-гейтов нет:
  `grep -rE 'feature = "tron"' crates/*/Cargo.toml` пуст). Live-тесты самоскипаются без
  `TRON_LIVE`. `ci.yml` тянет reusable-workflows `tempoxyz/ci`/`tempoxyz/gh-actions` (:153,186).
- Live-тесты, keyless+spendless (кандидаты в cron): `tron_mainnet_fork_reads_usdt`
  (`crates/forge/tests/cli/tron.rs`, mainnet /jsonrpc, читает + energy golden 1915);
  провайдерские read-пробы Nile `live_get_now_block`/`live_get_chain_id_nile`/
  `live_balance_and_txinfo`/`live_trigger_constant_total_supply` (`client.rs:544-666`) — но
  трогают `/wallet/*` (WAF-риск). Тратящие TRX (`live_deploy_counter_on_nile`,
  `live_golden_energy_parity_on_nile` и др., client.rs:781-1075) — НЕ для крона.
- `forge --version` без Tron-идентичности: строка из `crates/common/build.rs:38-46,63-69`
  (`FOUNDRY_SHORT_VERSION={version} ({sha} {ts})`) — единственный шов для штампа.
- `sandbox/tron-counter/README.md:64-67` устарел вдвойне: несуществующий путь `…/foundry/target/…`
  и описание stage-1.5 фейла `OpcodeNotFound (0xD3)`, давно починенного C2/E.
- Release: tron безусловен (не фича) ⇒ дрейфа Makefile↔release.yml не добавляет; `release.yml`
  использует `${{ github.repository }}` (fork-safe). Docker/installer захардкожены на foundry-rs,
  но foundryup параметризован `FOUNDRYUP_REPO` — документировать, не менять.

---

## Задачи

Порядок: H1 → H2 → Fable-ревью(H1+H2) → H3 → H4 → Fable-ревью(H3+H4) → H5 → H6 → финальное
Fable-ревью этапа. Каждая задача — отдельный конвенциональный коммит(ы).

### H1 — Fork-guard: bail на явный исторический блок (S)

1. Pre-flight в forge test cmd (рядом с `infer_network_from_fork`): если tron-путь И пользователь
   явно передал `--fork-block-number` — `bail!` с текстом вида: «Tron forks are tip-only: state is
   served only at the chain tip (/jsonrpc serves state only at TAG latest). Drop
   --fork-block-number for a tip fork.» Ключевать на СЫРОМ значении до авто-пина `get_fork`.
2. Гард в `crates/cheatcodes/src/evm/fork.rs`: `_1`-варианты `createFork`/`createSelectFork`
   с явным `blockNumber` на tron → ошибка чит-кода с тем же объяснением. Безблочные варианты не
   задеты.
3. Тесты: (a) CLI-тест `forge test --fork-url … --fork-block-number N` на tron → assert_failure с
   этим сообщением (оффлайн: до сети дойти не должно — bail до fork-конструкции; проверить, что
   ошибка не требует живого URL); (b) чит-код-вариант; (c) tip-fork без блока остаётся зелёным
   (live-гейт `TRON_LIVE=1` — существующий `tron_mainnet_fork_reads_usdt` не задет).
4. Спека §4.4: формулировка уже обещает явную ошибку — синхронизировать STATUS (снять follow-up).

### H2 — Gas-report: energy + bandwidth (M)

1. `GasReport` получает tron-флаг (через `GasReport::new`/`with_network` на
   test/mod.rs:2762-2768) + нужные поля `config.tron` (fee_limit и др. для оценки).
2. Bandwidth-оценщик: предпочтительно реюз билдеров protobuf (`build_trigger_raw`/
   `build_create_raw` + 65-байтовая dummy-подпись + плейсхолдерные TAPOS/timestamp) →
   `encode_to_vec().len() + 64`. Проверить dep-направление `forge → foundry-tron-provider`
   (или перенести/продублировать билдеры тонким слоем в primitives, если цикл; НЕ дублировать
   раскладку полей вручную без крайней необходимости — closed-form только при цикле, с тестом
   равенства против настоящего билдера).
3. `analyze_node`: на tron считать bandwidth per frame (calldata для calls, initcode для creates,
   имя контракта из декодера) и хранить рядом с gas тем же `frames`-паттерном
   (min/avg/median/max для fuzz).
4. Рендер: таблица — заголовки `Energy`/`Deployment Energy` + bandwidth-блок/колонки
   (layout-решение зафиксировать в коде комментарием); JSON — `Option`-поля с
   `#[serde(default, skip_serializing_if = "Option::is_none")]`, эмит только на tron.
5. Тесты: (a) оффлайн юниты оценщика против committed-фикстур: trigger-фикстура → **345**,
   create-фикстура → **853** (точные значения, без допусков); (b) CLI-тест tron gas-report
   (таблица + `--json`) по образцу cmd.rs:2039 на sandbox-стиле Counter (tron-solc из кэша;
   если clean-CI без бинарника — гейт или фикстурный путь, НЕ ослаблять); (c) EVM gas-report
   снапшоты до/после БАЙТ-ИДЕНТИЧНЫ (прогнать существующие снапшот-тесты).
6. `forge snapshot` не трогать. Per-test network override + `--gas-report` — вне поддержки
   (гейт на run-level is_tron).

### H3 — TRONSCAN verify: провайдер + standalone `forge verify-contract` (M/L)

1. Пины long-version: `crates/tron/solc/src/pins.rs` — commit/longVersion per версия из
   list.json; экспорт `tronscan_compiler_string(&Version)`; юнит против live-подтверждённого
   `tron_v0.8.25+commit.77bd169f` (и против list.json для 26/27).
2. `VerificationProviderType::Tronscan` (enum/FromStr/Display/client) + новый модуль
   `crates/verify/src/tronscan/mod.rs` с `VerificationProvider`.
3. Ранний tron-диспетч в `VerifyArgs::run` (зеркало create.rs:154) до alloy-резолюции; форс
   Tronscan-провайдера; skip aux-Sourcify в `collect_runs`. Routing хостов: mainnet
   728126428 → apilist.tronscanapi.com, nile 3448148188 → nileapi.tronscan.org, оверрайд
   `--verifier-url`; chain id брать из конфига/RPC tron-путём; неопределим → понятная ошибка с
   просьбой `--verifier-url`. Решение зафиксировать в коде+доке.
4. Submit: multipart-форма по F-5 (contractAddress=to_base58(addr), contractName, compiler из
   пинов, optimizer/runs/evmVersion/viaIR из project settings, license из `license_type`,
   constructorParams=hex без 0x, files=флаттеный исходник через `foundry_common::flatten`,
   vanilla dry-run и ipfs-ассерт пропущены). Парсинг `{code,errmsg,data.status}`:
   2006→успех, 2001→«already verified» (не ошибка), 2007/2008/прочее→ошибка с errmsg.
   Retry-обёртка как в etherscan-провайдере.
5. `check`: re-query `/api/solidity/contract/info`, `data.status==2` ⇒ verified; вывести URL
   странички контракта.
6. Cargo: reqwest+`multipart`, `foundry-tron-primitives`, `foundry-tron-solc` в crates/verify.
7. Тесты: оффлайн юниты — сборка формы (все поля, hex без 0x, base58-адрес), маппинг статусов,
   host-routing, compiler-string; гейт `TRON_LIVE=1` — re-query `/info` известного verified
   контракта `TMv7hAfswe2EvXG4nUeNFGEgNWE8Joedtu` (mainnet, read-only) с ассертом формата
   compiler-строки. **Acceptance-гейт (live, тратит TRX)**: один реальный E2E на Nile —
   задеплоить свежий Counter (`forge create`) и верифицировать его через новый путь
   (`forge verify-contract`), добиться `2006`/`status==2`; выполнить ОДИН РАЗ при имплементации
   (ключ `.env.tron-dev`), в тест оформить под гейт `TRON_LIVE=1` + отдельная env
   (`TRON_VERIFY_E2E=1`), НЕ для крона. Если TRONSCAN отвергнет compiler-строку 0.8.27 —
   прочитать `/info` любого 0.8.27-соседа и скорректировать формат (пины правятся, тест
   обновляется — это предусмотренный сценарий, не провал).

### H4 — Embedded verify + address-полировка (S/M)

1. Разблокировать `forge create --verify` на tron (`create.rs:189-197`): конструировать
   `VerifyArgs` из `CreateArgs` (как generic-путь) и звать Tron-провайдера; `--unlocked`/
   `--browser` остаются bail. `forge script --verify` — если шов симметричен, разблокировать
   тем же способом; если нет — оставить bail с точной причиной и записать follow-up в STATUS.
2. Принимать `T…`/`41…` в адресном аргументе `forge verify-contract` (парсер уже есть:
   `address.rs:80`); без ломки `0x`-пути.
3. Тесты: юнит на парс адреса; CLI-тест, что `forge create --verify` на tron больше не bail'ится
   на этапе валидации аргументов (сам сабмит — под live-гейтом); non-tron `--verify` не задет.

### H5 — Документация + hygiene (M)

1. `docs/tron/USER_GUIDE.md` (английский, self-contained): quickstart (`network = "tron"`,
   `[rpc_endpoints]`, авторезолв tron-solc, sandbox-walkthrough); матрица покрытия команд
   (works / explicit error / out-of-scope — по F-7 и STATUS, включая новое: gas-report,
   verify-contract, fork-guard); справочник `[tron]`-конфига (таблица из F-7) + пер-командные
   оверрайды; форматы адресов; отличия VM/energy-модели (CREATE-схема, depth 64 vs 1024,
   ISCONTRACT-стаб, bandwidth-оценка «для foundry-broadcast tx»); fork-режим (tip-only,
   mainnet-only /jsonrpc, read-only, явная ошибка на исторический блок); верификация TRONSCAN
   (keyless, flattened, хосты, license-коды); env-гейты live-тестов; foundryup из форка
   (`FOUNDRYUP_REPO=elsvv/foundry-tron`).
2. Root `README.md`: короткая секция «Tron support» со ссылкой на USER_GUIDE (стоковый текст
   upstream не перелопачивать).
3. `sandbox/tron-counter/README.md`: убрать мёртвый путь `…/foundry/target/…` и устаревший
   «Reproduce OpcodeNotFound» — заменить актуальным walkthrough (build/test через авторезолв).
4. Version-stamp: `crates/common/build.rs` — суффикс tron-форка в `forge --version`
   (например `1.7.2-tron.<sha>` или `(tron; <sha> <ts>)` — выбрать минимально-инвазивный формат,
   не ломающий парсеры версии в тестах; прогнать затронутые снапшот-тесты).
5. Каждый доменный факт в доке сверять со STATUS/спекой/кодом — не выдумывать; коды лицензий,
   хосты, лимиты — из F-5/F-7.

### H6 — CI: tron-live workflow + follow-ups (S/M)

1. Новый `.github/workflows/tron-live.yml`: schedule (еженедельный cron) + `workflow_dispatch`;
   один job, non-blocking по духу (отдельный workflow и так не гейтит PR); `TRON_LIVE=1`, БЕЗ
   ключей; запускает keyless+spendless набор, якорь — `tron_mainnet_fork_reads_usdt`
   (nextest-фильтр по имени). Nile `/wallet`-пробы в крон НЕ включать (WAF) — опционально
   отдельный manual-only job с секретами `TRON_PRO_API_KEY`/`TRON_PRIVATE_KEY` для полного
   голдена (`workflow_dispatch` only, с комментарием про faucet).
2. Сборка в job: `cargo build -p forge --profile <существующий в ci>` или реюз паттернов
   test.yml — минимальный самостоятельный job, НЕ перепахивать существующую матрицу (tron-оффлайн
   уже в ней). Учесть, что reusable tempoxyz-workflows могут быть недоступны — новый workflow
   самодостаточен (checkout + rustup + nextest run с фильтром).
3. Валидация: `actionlint` если доступен (или ручная YAML-проверка + `gh workflow list` после
   пуша — это уже оркестратор); зафиксировать в STATUS, что первый полный прогон CI форка —
   отдельное событие после пуша (0 runs до сих пор), возможные латентные красноты чинятся
   follow-up'ом.
4. STATUS.md: обновить (план H готов, секции по каждой задаче, находки, DEFER-лист), сохранив ВСЕ
   прежние находки. Обновить «Следующие планы». Спека: §4.4 (gas-report реализован, формулировка
   fork), §4.8 (verify реализован) — отметки «реализовано (план H)» в стиле прежних правок.

---

## Вне скоупа (DEFER, зафиксировать в STATUS)

- Вынос tron-крейтов в отдельные репо: tron-логика энтэнглена (evm/core inline-модули
  `evm/tron/*`, inline `tron.rs` в cast/config/script/cli) — вынос при реальном OSS-релизе.
- Публикация бинарных релизов / Docker / rebranding installer'а — после первого зелёного CI и
  живой верификации; foundryup уже параметризован env'ом.
- Standard-json / multi-file верификация TRONSCAN (не доказана), верификация через `forge script
  --verify` если шов кривой (тогда follow-up).
- Скраб русских внутренних доков — USER_GUIDE их суперсидит для пользователей.

## Риски

- R1: EVM-снапшоты gas-report — любое безусловное изменение ломает их; всё под is_tron-гейтом,
  верификатор диффит EVM-выход до/после байт-в-байт.
- R2: dep-цикл forge → tron-provider для билдеров bandwidth; fallback — тонкий слой в primitives.
- R3: compiler-строки 0.8.26/27 live-сабмитом не проверены — acceptance-гейт H3.7; при отказе
  формат читается из `/info` соседа.
- R4: TRONSCAN rate-limit/WAF — один POST + retry, риск низкий.
- R5: CI форка никогда не бегал — латентные красноты матрицы не в скоупе H6 (фиксится
  follow-up'ом после пуша); H6 отвечает только за корректность нового workflow-файла.
- R6: `forge script --verify` шов может не лечь зеркально create — предусмотрен выход bail+follow-up.
