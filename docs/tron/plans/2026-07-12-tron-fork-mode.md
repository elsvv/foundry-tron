# Tron Fork Mode Plan (План G — Этап 2, read-only fork через /jsonrpc)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `forge test --fork-url <java-tron /jsonrpc>` при `network = "tron"` — read-only fork реального состояния Tron (mainnet) для тестов: балансы/код/сторадж реальных контрактов в tron-revm с верной energy-моделью.

**Разведка:** scout-stage2/fork-mode.md (2026-07-12), все методы /jsonrpc опробованы live. Ветка **`tron-stage2`** (поверх планов E/F).

## Верифицированные доменные факты

1. **Единственный блокер**: java-tron `/jsonrpc` НАМЕРЕННО отдаёт `-32601` на `eth_getTransactionCount` (постоянный стаб, `TronJsonRpc.java:251-256` — аннотация JsonRpcMethodNotFoundException как единственное поведение). fork-db `get_account_req` делает `try_join3(balance, nonce, code)` → любой account-load падает. Всё остальное, что нужно fork-backend'у, работает live: eth_chainId, getBlockByNumber/Hash (full и hashes), getCode, getBalance, getStorageAt, getLogs (на TronGrid), getTransactionReceipt/ByHash, estimateGas (в energy).
2. **Nonce=0 безопасен для Tron**: у аккаунтов нет EVM-nonce (деплой — txid-схема); foundry инкрементирует in-memory от загруженной базы. Read-only fork ничего не теряет.
3. **Форма блоков**: `hash` = tron block id (первые 8 байт — номер блока) — это КОРРЕКТНАЯ Tron-семантика BLOCKHASH (java-tron сам так отвечает); `stateRoot: "0x"` — alloy 2.1.1 парсит через lenient_state_root → ZERO; `baseFeePerGas: 0x0`; адреса в ответах 20-байтовые 0x (0x41 уже снят) — ремап не нужен.
4. **Точки провайдера**: `crates/common/src/provider/mod.rs` — `ClientBuilder::default().layer(retry_layer).transport(...)` (~L395 и ~L410) — место для tower-layer; `fork_provider_with_url` (crates/evm/core/src/opts.rs:121-131); `create_fork` (crates/evm/core/src/fork/multi.rs:585-617).
5. **Stage-1 гейты**: `crates/script/src/lib.rs:293-297` — script отклоняет fork для tron (bail на fork_block_number, затем fork_url=None) — для script ОСТАВИТЬ (broadcast-контекст, вне скоупа G); **forge test гейта не имеет** — уже строит fork генерически (`test/mod.rs:2451` get_fork безусловно) и умирает на nonce; после шима работает почти без доводки.
6. **Эндпоинты**: `/jsonrpc` есть на `api.trongrid.io` (mainnet, live-проверен); на `api.nileex.io` НЕ смонтирован (404); nile.trongrid.io с этой машины недоступен → live fork-тесты — **mainnet, read-only** (без ключа, умеренный rate).
7. `net_version` у java-tron возвращает hex (нестандарт) — chain-id брать только через eth_chainId (alloy так и делает).

## Global Constraints

- Ветка `tron-stage2`; всё как в плане E Global Constraints (тулчейн, конвенции, коммиты, запреты на ослабление тестов).
- Тесты с форком обязаны содержать `fork` в имени (правило репо). Live fork-тесты — гейт `TRON_LIVE=1` (read-only mainnet, без трат TRX), с явным skip-eprintln.
- foundry-fork-db (git-pinned) НЕ патчить — только слой на нашей стороне.
- Ничего не менять для non-tron сетей: слой применяется строго при `is_tron()`.

### Task G1: nonce-шим и включение fork-пути для tron

**Files:** Modify `crates/common/src/provider/mod.rs` (tower-layer + опция в ProviderBuilder), `crates/evm/core/src/opts.rs` / `fork/multi.rs` (прокладка флага tron до построения провайдера — минимальный маршрут выяснить по месту: где EvmOpts/NetworkConfigs доступны при создании fork-провайдера), возможно `crates/config` (прокидка). Tests: юнит в common + forgetest-style.

- [ ] Step 1: Падающий юнит-тест слоя: сервис-мок (tower) — запрос `eth_getTransactionCount` любых параметров → ответ `"0x0"` БЕЗ обращения к внутреннему транспорту; любой другой метод — проходит насквозь. (Форма: RequestPacket/ResponsePacket alloy-transport; посмотреть, как устроен retry_layer рядом — писать в том же стиле.)
- [ ] Step 2: Реализация `TronNonceShimLayer` (имя по вкусу кода) + включение: `ProviderBuilder` получает флаг (builder-метод `.tron_shim(bool)` или через существующий config-параметр) — применяется в обоих местах постройки клиента; протянуть от `NetworkConfigs::is_tron()` в местах, где строится fork-провайдер (get_fork/create_fork/fork_evm_env путь). Убедиться: обычные (не-fork) tron-провайдеры не задеваются (или задеваются безвредно — shim безопасен всюду на tron-пути; решить по месту, задокументировать).
- [ ] Step 3: Понятная ошибка вместо глубокой DatabaseError: если network=tron и fork_url задан, но eth_chainId на нём падает (например, дали /wallet-хост) → диагностика «tron fork requires the /jsonrpc endpoint (e.g. https://api.trongrid.io/jsonrpc)». Место — ранняя валидация в get_fork/фор-настройке для tron.
- [ ] Step 4: `cargo test -p foundry-common` (+ затронутые), `cargo check --workspace`, clippy/fmt; commit `feat(tron): nonce shim enabling read-only fork over /jsonrpc`.

### Task G2: live fork-тесты + доки

**Files:** Create fork-тест (место по конвенции: `crates/forge/tests/it/` или рядом с tron-тестами — где уже есть fork-сьюты, использовать их паттерн + `fork` в имени), Modify `docs/tron/STATUS.md`, спека (отметка fork-режима), `sandbox/tron-counter/README.md` (пример команды).

- [ ] Step 1: Live fork-тест(ы) (TRON_LIVE=1, mainnet `https://api.trongrid.io/jsonrpc`): солидити-тест или executor-тест, который на форке читает РЕАЛЬНЫЙ mainnet-контракт: USDT TRC-20 (`TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t` = 0x41-снятый hex `a614f803b6fd780986a42c78ec9c7f77e6ded13c`): `name()` == "Tether USD", `decimals()` == 6, `totalSupply() > 0`, `balanceOf(известный богатый адрес) > 0`; плюс чтение кода (extcodesize > 0) и конкретного storage-слота. Ассерты на РЕАЛЬНЫЕ стабильные значения (name/decimals — константы контракта; supply/balance — `> 0`, не точные). Проверить и `vm.createSelectFork` путь, если он проходит через тот же провайдер.
- [ ] Step 2: Убедиться, что energy-модель работает на форке (тест меряет gas_used вызова name() на форке — sanity: число в тронских масштабах, зафиксировать точное значение если детерминировано).
- [ ] Step 3: script-путь: подтвердить, что stage-1 режекция для script сохранена и сообщение актуально («use forge test for fork; script broadcast on fork unsupported»). forge test без fork-url — не задет (офлайн-сьюты).
- [ ] Step 4: STATUS.md (план G ✅, mainnet-only live-канал, nileex без /jsonrpc), спека — отметить fork-режим реализованным (read-only), README-пример. Полный критерий: `cargo check --workspace`, оффлайн-сьюты зелёные, live fork-тест зелёный. Commits: `test(tron): mainnet fork read-only coverage` + `docs(tron): stage-2 fork mode done`.

## Критерий завершения плана G

Юнит шима зелёный; live: `forge test` c fork-url mainnet /jsonrpc проходит USDT-тесты (name/decimals/supply/balance/код/сторадж); понятная ошибка на неверном эндпоинте; non-tron пути не задеты; `cargo check --workspace` чистый; STATUS/спека обновлены.
