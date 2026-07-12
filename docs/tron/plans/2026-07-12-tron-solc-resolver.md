# tron-solc Auto-Resolver Plan (План F — Этап 2, спека §4.3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** при `network = "tron"` и незаданном `solc` — автоматическое скачивание и pin-верификация нативного tron-solc (как svm для ванильного solc), чтобы `foundry.toml` проектов не содержал машинно-зависимых абсолютных путей. Sandbox переводится на авторезолв.

**Разведка:** scout-stage2/solc-resolver.md (2026-07-12), факты проверены live (gh api, solc-bin list.json). Ветка **`tron-stage2`** (поверх плана E).

## Верифицированные доменные факты

1. **svm непригоден**: `SOLC_RELEASES_URL` захардкожен на binaries.soliditylang.org (svm-rs releases.rs:17), хуков переопределения нет; `Solc::verify_checksum` сверяет с soliditylang-списком (гарантированный mismatch для tron-solc) — НЕ вызывать на tron-бинарях.
2. **Релизы tron-solc**: репо `tronprotocol/solidity`, теги `tv_0.8.X` (…, tv_0.8.25, tv_0.8.26, tv_0.8.27 = latest). Ассеты (0.8.20+, имена БЕЗ версии): `solc-macos` (universal fat: x86_64+arm64), `solc-static-linux` (x86_64), `solc-windows.exe`. URL: `https://github.com/tronprotocol/solidity/releases/download/tv_{ver}/{asset}`. Linux-ARM бинарей НЕТ (known gap).
3. **Чексуммы**: `https://tronprotocol.github.io/solc-bin/{platform}/list.json`, platform ∈ {linux-amd64, macosx-amd64, windows-amd64} (arm-платформ нет — macOS ARM покрыт universal-бинарём под macosx-amd64-списком). Формат: `{"builds":[{path,version,build,longVersion,keccak256,sha256,urls:[]}]}` — только таблица хэшей (не svm Releases: нет releases-map, urls пустые). Проверенный pin: 0.8.27 macos `sha256 = 9e369b442b3a835320cf1450a21ce5e8acf0d319a50d10a10a2001aac7ce17aa` (совпадает со Stage-1 бинарём на этой машине).
4. **Шов**: `Config::ensure_solc` (crates/config/src/lib.rs:~1452) — обрабатывает `SolcReq::Version` (svm) / `SolcReq::Local` / `None` (→ AutoDetect). Возврат — через **`Solc::new_with_version(path, version)`** (без exec `--version` и без soliditylang-checksum). `self.offline` уже учитывается. foundry-config НЕ имеет reqwest/sha2/tokio → скачивание живёт в отдельном крейте.
5. **Stage-1 конвенция**: `~/.foundry-tron/solc/tron-solc-{ver}` (не кодифицирована — кодифицировать в новом крейте). `SolcReq::Local` не разворачивает `~` → абсолютные пути.
6. tron-solc `--version` даёт `0.8.27+commit.19164bed.Darwin.appleclang`; `evm_version cancun` принимается (sandbox работает со Stage-1).

## Global Constraints

- Ветка `tron-stage2`, коммиты локально, НЕ пушить. Тулчейн/лимиты/конвенции — как в плане E (`docs/tron/plans/2026-07-12-tron-revm-full.md` Global Constraints).
- Никаких новых внешних зависимостей: HTTP — `reqwest` (уже в workspace; блокирующий контекст — через `foundry_compilers`-паттерн `RuntimeOrHandle::block_on` либо reqwest::blocking, если фича уже доступна транзитивно — сверить по месту), хэш — `sha2` (в workspace).
- Скачивание НИКОГДА не происходит при `offline = true` и при наличии кэша; sha256-верификация обязательна, mismatch = ошибка с удалением файла.
- Download-тест — гейт `TRON_SOLC_DOWNLOAD=1` (сеть медленная, бинарь 80 МБ) с явным eprintln-skip; остальные тесты оффлайн (кэш-хит, URL/платформа, таблица пинов, отказ при offline).

### Task F1: крейт `foundry-tron-solc`

**Files:** Create `crates/tron/solc/` (Cargo.toml, src/lib.rs, src/pins.rs); Modify корневой Cargo.toml (member + workspace.dependencies).

- [ ] Step 1: Падающие тесты: `default_version()` = 0.8.27; `binary_path(ver)` = `~/.foundry-tron/solc/tron-solc-{ver}` (абсолютный); `asset_name()` по платформе (macos → solc-macos, linux-x86_64 → solc-static-linux, windows → solc-windows.exe, linux-arm → понятная ошибка); `pinned_sha256(ver, platform)` из встроенной таблицы (как минимум 0.8.25/0.8.26/0.8.27 — значения ВЗЯТЬ из живого list.json и зафиксировать); кэш-хит: существующий файл с верным sha256 → путь без сети; повреждённый кэш (неверный sha) → ошибка с указанием пересборки; `offline=true` без кэша → ошибка "offline".
- [ ] Step 2: Реализация `resolve_tron_solc(version: &Version, offline: bool) -> Result<PathBuf, TronSolcError>`: кэш-проверка (файл + sha256) → скачивание (redirect-friendly, таймауты, ретраи ×3) → sha256-верификация → атомарная запись (tmp + rename) + chmod 755 → путь. Таблица пинов `pins.rs` — `const`, сгенерированная из list.json (комментарий-источник с датой). Download-гейт-тест: реально качает САМЫЙ МАЛЫЙ ассет-вариант для текущей платформы одной версии и сверяет sha256 (гейт TRON_SOLC_DOWNLOAD=1).
- [ ] Step 3: тесты/clippy/fmt; commit `feat(tron): tron-solc resolver crate with pinned checksums`.

### Task F2: интеграция в Config + sandbox на авторезолв

**Files:** Modify `crates/config/Cargo.toml` (+foundry-tron-solc), `crates/config/src/lib.rs` (`ensure_solc`), `sandbox/tron-counter/foundry.toml` (убрать абсолютный путь), `sandbox/tron-counter/README.md`, `docs/tron/STATUS.md`.

- [ ] Step 1: Падающий тест конфига: `network = "tron"` без `solc` → `ensure_solc` возвращает Solc с путём `~/.foundry-tron/solc/tron-solc-0.8.27` и version 0.8.27 (при наличии кэша на машине — есть); `solc = "0.8.26"` + tron → резолв 0.8.26 (тест через мок кэша: подготовить файл с верным pin? — sha256 0.8.26 неизвестен локально: положить фиктивный файл нельзя (sha провалится) → тест этого арма через download-гейт ИЛИ юнит на выбор версии без фактического резолва — выбрать честный вариант, не ослаблять); `solc = "/abs/path"` → как раньше (Local, без резолвера); non-tron → поведение не изменилось (существующие тесты).
- [ ] Step 2: Реализация в `ensure_solc`: ветка `self.networks.is_tron()`: None → default_version; Version(v) → v; резолв через foundry-tron-solc (уважая `self.offline`), возврат `Solc::new_with_version`. Local — без изменений. Убедиться, что `is_auto_detect()` не ломает путь (tron + None solc → теперь НЕ AutoDetect).
- [ ] Step 3: Sandbox: `foundry.toml` — удалить `solc = "/Users/..."`; `rm -rf out cache && forge build && forge test` → 5/5 через авторезолв (кэш-хит). README обновить.
- [ ] Step 4: `cargo test -p foundry-config`, `cargo check --workspace`, clippy/fmt; STATUS.md (план F ✅, конвенция кодифицирована, linux-arm gap); commit `feat(tron): auto-resolve tron-solc from config`.

## Критерий завершения плана F

Оффлайн: foundry-config тесты зелёные, sandbox 5/5 без абсолютного пути solc; `cargo check --workspace` чистый; download-гейт-тест работает (прогнать один раз при живой сети); STATUS.md обновлён.
