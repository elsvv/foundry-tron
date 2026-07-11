# foundry-tron: статус проекта

Форк Foundry с поддержкой Tron (TVM). Fork: `elsvv/foundry-tron`, upstream: `foundry-rs/foundry` (в апстриме уже есть мультисетевость Ethereum/Optimism/Tempo — Tron добавляется по тем же швам). Рабочая ветка: **`tron-dev`**.

Обновлено: 2026-07-11.

## Документы

- Спека (утверждена): `docs/tron/specs/2026-07-10-foundry-tron-fork-design.md`
- Планы: `docs/tron/plans/` — один план на подсистему, исполняются последовательно.

## Состояние этапов

| План | Что | Статус |
|---|---|---|
| A | `crates/tron/primitives` — адресный кодек base58check/0x41, protobuf-транзакции (txID=sha256(raw_data)), TAPOS, подпись secp256k1 (65 байт, v=27+recid) | ✅ Готов. 18 тестов. Смоук на Nile: tx `3a4d9c5f…` в блоке 69090417 |
| B | `crates/tron/provider` — async HTTP-клиент `/wallet/*`: блоки/TAPOS, балансы, tx-info, constant-вызовы + energy, broadcast, `send_transfer` | ✅ Готов. 15 тестов (оффлайн fixtures + live `TRON_LIVE=1`), E2E на Nile |
| C | `NetworkVariant::Tron`, маркер `TronEvmNetwork` (Network=Ethereum, EvmFactory=EthEvmFactory), диспетчеризация forge test, clippy-чистка | ✅ Задачи 1–3 готовы. Задача 4 (E2E-гейт) заблокирована → C2 |
| C2 | Мини tron-revm: `TronEvmFactory` (EthEvmFactory + insert_instruction для 0xD0–0xD4), закрытие E2E-гейта плана C | 📝 План готов: `docs/tron/plans/2026-07-11-tron-mini-revm.md` (семантика/energy сверены с java-tron). **Workflow НЕ запускать без команды пользователя** |
| D | cast/forge script: деплой на Nile через tron-provider | ⏳ После C2 |
| Этап 2 | Полный tron-revm: precompiles (0x09 BatchValidateSign, Ripemd160→0x20003, Blake2F→0x20009), CREATE2-префикс 0x41, energy/bandwidth-репорт, резолвер tron-solc | ⏳ |

## Ключевые находки (не потерять)

1. **tron-solc вставляет TVM-опкоды `0xD3`/`0xD2` (CALLTOKENID/CALLTOKENVALUE) в non-payable guard каждого контракта** → байткод tron-solc не исполняется на ванильном revm (`OpcodeNotFound` в конструкторе). Флага отключения нет. Отсюда план C2.
2. **Нативные бинарники tron-solc существуют**: github.com/tronprotocol/solidity/releases, ассет `solc-macos` (Intel, на Apple Silicon — через Rosetta 2), версии до 0.8.27_Democritus_v4.8.1. Скачан в `~/.foundry-tron/solc/tron-solc-0.8.27`. TronBox качает только wasm — нативные лежат именно в релизах.
3. Chain id Tron mainnet: `728126428`. TVM ≈ Cancun (java-tron 4.8.x): PUSH0, TLOAD/TSTORE, MCOPY есть; BLOBHASH/BLOBBASEFEE — заглушки 0.
4. Транзакции — protobuf (не RLP), txID = sha256(raw_data), подпись по txID, TAPOS вместо nonce, `eth_sendRawTransaction` отсутствует — запись только через HTTP `/wallet/*` (см. crates/tron/provider).
5. Sample-проект: `sandbox/tron-counter` (без forge-std; ассерты chainid=728126428 и tstore/tload).

## Окружение (важно для любой машины)

- **Тулчейн:** зависимости требуют rustc ≥1.91. Если системный rustc старее (на исходной машине Homebrew 1.88 перекрывал rustup), все cargo-команды запускать так:
  ```bash
  TC=$(dirname "$(rustup which --toolchain stable cargo)"); PATH="$TC:$PATH" cargo <...>
  ```
  Форматирование — nightly rustfmt напрямую (`~/.rustup/toolchains/nightly-*/bin/rustfmt --edition 2024 --config-path rustfmt.toml`).
- **Live-тесты:** `TRON_LIVE=1` + `TRON_PRIVATE_KEY` (файл `.env.tron-dev` в корне репо, в git НЕ входит — перенести вручную или сгенерировать новый ключ и пополнить через кран https://nileex.io/join/getJoinPage). Текущий тестовый адрес: `TX7izXWcmofRYonzdcThrS78jifMtVWCuf` (~1997 TRX на Nile).
- Тесты tron-крейтов: `cargo test -p foundry-tron-primitives -p foundry-tron-provider`. CI-линт: `cargo clippy --all-targets` с `-Dwarnings` — tron-крейты чистые.

## Процесс работы (утверждено пользователем)

- Планы исполняются dynamic-workflow: исполнители Opus → Opus-верификатор после каждой задачи (fix-цикл до 3) → Fable-ревью после каждых двух задач и в конце (тоже с fix-циклом). Шаблон скрипта — в планах/истории сессий.
- Требования к тестам: только реальные векторы/fixtures (сеть, официальные .proto, java-tron), никакой тавтологии, запрещено ослаблять ассерты и ставить `#[ignore]`; live-тесты гейтятся env-переменной с явным `eprintln("skipped…")`.
- Верификаторы обязаны независимо перепроверять доменные факты (повторные curl, пересчёт векторов, сверка с первоисточниками) — этот паттерн уже поймал 2 неверных вектора в планах и находку №1.
