# tron-provider Implementation Plan (План B этапа 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Крейт `foundry-tron-provider` — async HTTP-клиент Tron-нод (TronGrid/java-tron `/wallet/*`): чтение (блоки, аккаунты, tx-info), constant-вызовы с оценкой energy, broadcast подписанных транзакций и high-level отправка с TAPOS+подтверждением, поверх `foundry-tron-primitives`.

**Architecture:** Тонкий типизированный клиент на reqwest (async, tokio) поверх нативного HTTP API Tron. Никакого `eth_*` JSON-RPC в этом крейте (маппинг на alloy Provider — планы C/D). Все адреса на входе/выходе API — 20-байтовый `Address`; конверсия в hex41 — через `foundry-tron-primitives`. Десериализация проверяется на сохранённых реальных ответах Nile (fixtures), сетевое поведение — live-тестами против Nile, включаемыми `TRON_LIVE=1`.

**Tech Stack:** reqwest (workspace), tokio (workspace, `#[tokio::test]`), serde/serde_json, foundry-tron-primitives (план A, проверен смоуком: Nile принял транзакцию, блок 69090417).

**Спека:** `docs/superpowers/specs/2026-07-10-foundry-tron-fork-design.md` (секция 4.5).

## Global Constraints

- Все пути — относительно `/Users/vaceslaveliseev/@dev/foundry-tron/foundry/`; работаем в ветке `tron-dev` (уже существует, там план A).
- **Тулчейн (важно, иначе сборка падает):** дефолтный Homebrew rustc 1.88 в PATH слишком стар (alloy требует ≥1.91). Все cargo-команды запускать так: `TC=$(dirname "$(rustup which --toolchain stable cargo)"); PATH="$TC:$PATH" cargo <...>`. Форматирование — nightly rustfmt: `~/.rustup/toolchains/nightly-*/bin/rustfmt --edition 2024 --config-path rustfmt.toml <files>`.
- Тесты: `PATH="$TC:$PATH" cargo test -p foundry-tron-provider`. Live-тесты дополнительно требуют `TRON_LIVE=1` и `TRON_PRIVATE_KEY` (лежит в `/Users/vaceslaveliseev/@dev/foundry-tron/.env.tron-dev`; адрес `TX7izXWcmofRYonzdcThrS78jifMtVWCuf` профинансирован ~1999 TRX на Nile).
- Live-тесты БЕЗ `#[ignore]`: гейт — явный early-return с `eprintln!("skipped: set TRON_LIVE=1 ...")` при отсутствии env. Это осознанный документированный паттерн (сетевые тесты не должны ронять оффлайн-CI), не ослабление.
- Fixtures — только реальные ответы Nile/mainnet, скачанные curl-командами из шагов плана. Выдумывать JSON запрещено.
- Внутри API — 20-байтовый `Address`; hex41/base58 только через `foundry_tron_primitives::address`.
- Коммиты — conventional commits `feat(tron):` / `test(tron):`, внутри foundry-репо.
- Референсы поведения при сомнениях: docs TronGrid/`developers.tron.network` (HTTP API), код TronBox `src/components/TronWrap/index.js` (polling-паттерны).

---

### Task 1: Каркас крейта, TronProvider, get_now_block + tapos

**Files:**
- Modify: `Cargo.toml` (workspace members: добавить `"crates/tron/provider/"` рядом с `"crates/tron/primitives/"`)
- Create: `crates/tron/provider/Cargo.toml`
- Create: `crates/tron/provider/src/lib.rs`
- Create: `crates/tron/provider/src/client.rs`
- Create: `crates/tron/provider/testdata/nile_getnowblock.json`

**Interfaces:**
- Consumes: `foundry_tron_primitives::tapos::{ref_block, RefBlock}`
- Produces:
  - `TronProvider::new(base_url: &str) -> Result<Self, TronError>`
  - `TronProvider::with_api_key(self, key: String) -> Self` (header `TRON-PRO-API-KEY`)
  - `TronProvider::get_now_block(&self) -> Result<NowBlock, TronError>`; `NowBlock { pub block_id: [u8; 32], pub number: i64, pub timestamp_ms: i64 }`
  - `TronProvider::tapos(&self) -> Result<(RefBlock, i64), TronError>` — RefBlock + now_ms (timestamp свежего блока; часы машины не используем)
  - `TronError` (thiserror: `Http(reqwest::Error)`, `Api { code: String, message: String }`, `Decode(String)`, `Timeout(String)`)
  - приватный helper `post_json<T: DeserializeOwned>(&self, path: &str, body: serde_json::Value) -> Result<T, TronError>`

- [ ] **Step 1: Workspace + Cargo.toml крейта**

Добавить member в корневой `Cargo.toml`. Крейт (`crates/tron/provider/Cargo.toml`; package-поля по образцу `crates/tron/primitives/Cargo.toml`):

```toml
[package]
name = "foundry-tron-provider"
description = "Async HTTP client for Tron nodes (TronGrid / java-tron wallet API)"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
authors.workspace = true
license.workspace = true
homepage.workspace = true
repository.workspace = true

[lints]
workspace = true

[dependencies]
foundry-tron-primitives = { path = "../primitives" }
alloy-primitives.workspace = true
reqwest = { workspace = true, features = ["json"] }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["time"] }

[dev-dependencies]
alloy-signer-local.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

Если `foundry-tron-primitives` уже объявлен в `[workspace.dependencies]` — использовать `.workspace = true`; если нет — добавить туда `foundry-tron-primitives = { path = "crates/tron/primitives" }` и сослаться. Проверить фичи reqwest/tokio в workspace-декларации (`grep -n "reqwest\|^tokio" Cargo.toml`) и не дублировать уже включённые.

- [ ] **Step 2: Скачать fixture**

```bash
mkdir -p crates/tron/provider/testdata
curl -s -X POST https://nile.trongrid.io/wallet/getnowblock > crates/tron/provider/testdata/nile_getnowblock.json
python3 -c "import json;d=json.load(open('crates/tron/provider/testdata/nile_getnowblock.json'));print(d['blockID'],d['block_header']['raw_data']['number'])"
```

Expected: печатает 64-hex blockID и номер блока > 69_000_000.

- [ ] **Step 3: Падающие тесты**

`crates/tron/provider/src/client.rs`:

```rust
//! Typed async client for the Tron wallet HTTP API.

use alloy_primitives::hex;
use foundry_tron_primitives::tapos::{ref_block, RefBlock};
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum TronError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("node error {code}: {message}")]
    Api { code: String, message: String },
    #[error("decode error: {0}")]
    Decode(String),
    #[error("timeout: {0}")]
    Timeout(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NowBlock {
    pub block_id: [u8; 32],
    pub number: i64,
    pub timestamp_ms: i64,
}

pub struct TronProvider {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl TronProvider {
    pub fn new(base_url: &str) -> Result<Self, TronError> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: None,
            client: reqwest::Client::new(),
        })
    }

    pub fn with_api_key(mut self, key: String) -> Self {
        self.api_key = Some(key);
        self
    }

    async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, TronError> {
        todo!()
    }

    pub async fn get_now_block(&self) -> Result<NowBlock, TronError> {
        todo!()
    }

    pub async fn tapos(&self) -> Result<(RefBlock, i64), TronError> {
        todo!()
    }
}

/// Parses a raw `/wallet/getnowblock` JSON response.
pub(crate) fn parse_now_block(v: &serde_json::Value) -> Result<NowBlock, TronError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../testdata/nile_getnowblock.json")).unwrap()
    }

    #[test]
    fn parses_now_block_fixture() {
        let nb = parse_now_block(&fixture()).unwrap();
        let raw = &fixture()["block_header"]["raw_data"];
        assert_eq!(nb.number, raw["number"].as_i64().unwrap());
        assert_eq!(nb.timestamp_ms, raw["timestamp"].as_i64().unwrap());
        assert_eq!(hex::encode(nb.block_id), fixture()["blockID"].as_str().unwrap());
        assert!(nb.number > 69_000_000);
    }

    #[test]
    fn tapos_fields_derive_from_block() {
        let nb = parse_now_block(&fixture()).unwrap();
        let rb = ref_block(nb.number, &nb.block_id);
        // ref_block_bytes = big-endian(number)[6..8]
        let be = nb.number.to_be_bytes();
        assert_eq!(rb.bytes, be[6..8].to_vec());
        assert_eq!(rb.hash, nb.block_id[8..16].to_vec());
    }

    #[tokio::test]
    async fn live_get_now_block() {
        if std::env::var("TRON_LIVE").is_err() {
            eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
            return;
        }
        let p = TronProvider::new("https://nile.trongrid.io").unwrap();
        let nb = p.get_now_block().await.unwrap();
        assert!(nb.number > 69_000_000);
        assert_ne!(nb.block_id, [0u8; 32]);
    }
}
```

`src/lib.rs`:

```rust
//! # foundry-tron-provider
//!
//! Async HTTP client for Tron nodes (TronGrid / java-tron `/wallet/*` API).

mod client;

pub use client::{NowBlock, TronError, TronProvider};
```

- [ ] **Step 4: Убедиться, что падают**

Run: `TC=$(dirname "$(rustup which --toolchain stable cargo)"); PATH="$TC:$PATH" cargo test -p foundry-tron-provider`
Expected: FAIL на `todo!()`.

- [ ] **Step 5: Реализация**

```rust
async fn post_json<T: DeserializeOwned>(
    &self,
    path: &str,
    body: serde_json::Value,
) -> Result<T, TronError> {
    let mut req = self.client.post(format!("{}{path}", self.base_url)).json(&body);
    if let Some(key) = &self.api_key {
        req = req.header("TRON-PRO-API-KEY", key);
    }
    let value: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
    // Нода кладёт ошибки в тело с 200: {"Error": "..."} или {"code": "...", "message": "<hex>"}
    if let Some(err) = value.get("Error").and_then(|e| e.as_str()) {
        return Err(TronError::Api { code: "Error".into(), message: err.to_string() });
    }
    serde_json::from_value(value).map_err(|e| TronError::Decode(e.to_string()))
}

pub async fn get_now_block(&self) -> Result<NowBlock, TronError> {
    let v: serde_json::Value = self.post_json("/wallet/getnowblock", serde_json::json!({})).await?;
    parse_now_block(&v)
}

pub async fn tapos(&self) -> Result<(RefBlock, i64), TronError> {
    let nb = self.get_now_block().await?;
    Ok((ref_block(nb.number, &nb.block_id), nb.timestamp_ms))
}
```

```rust
pub(crate) fn parse_now_block(v: &serde_json::Value) -> Result<NowBlock, TronError> {
    let missing = |f: &str| TronError::Decode(format!("getnowblock: missing {f}"));
    let block_id_hex = v["blockID"].as_str().ok_or_else(|| missing("blockID"))?;
    let block_id: [u8; 32] = hex::decode(block_id_hex)
        .map_err(|e| TronError::Decode(e.to_string()))?
        .try_into()
        .map_err(|_| TronError::Decode("blockID is not 32 bytes".into()))?;
    let raw = &v["block_header"]["raw_data"];
    Ok(NowBlock {
        block_id,
        number: raw["number"].as_i64().ok_or_else(|| missing("number"))?,
        timestamp_ms: raw["timestamp"].as_i64().ok_or_else(|| missing("timestamp"))?,
    })
}
```

Примечание: `post_json` для getnowblock возвращает `serde_json::Value` — generic использоваться начнёт с Task 2; если clippy/компилятор ругается на неоднозначность, звать с турбофишем `self.post_json::<serde_json::Value>(...)`.

- [ ] **Step 6: Тесты зелёные (включая live)**

Run: `TC=...; PATH="$TC:$PATH" TRON_LIVE=1 cargo test -p foundry-tron-provider`
Expected: PASS (3 passed, live-тест реально сходил в Nile).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tron/provider
git commit -m "feat(tron): scaffold foundry-tron-provider with get_now_block and tapos"
```

---

### Task 2: get_account (баланс) и get_transaction_info

**Files:**
- Modify: `crates/tron/provider/src/client.rs`
- Create: `crates/tron/provider/testdata/nile_getaccount.json`
- Create: `crates/tron/provider/testdata/nile_txinfo.json`

**Interfaces:**
- Consumes: `foundry_tron_primitives::address::to_hex41`, `alloy_primitives::{Address, B256}`
- Produces:
  - `TronProvider::get_balance(&self, addr: Address) -> Result<u64, TronError>` — SUN; несуществующий аккаунт (пустой ответ `{}`) → `Ok(0)`
  - `TronProvider::get_transaction_info(&self, txid: B256) -> Result<Option<TxInfo>, TronError>` — `None`, пока транзакция не в блоке (нода отвечает `{}`)
  - `TxInfo { pub block_number: i64, pub fee_sun: u64, pub energy_used: u64, pub success: bool, pub contract_address: Option<Address> }` — `success` = поле `receipt.result` отсутствует (для TransferContract) или равно `"SUCCESS"`; `contract_address` парсится из hex41
  - `pub(crate)` парсеры `parse_account_balance`, `parse_tx_info` (тестируются на fixtures)

- [ ] **Step 1: Fixtures с Nile (реальные данные плана A!)**

```bash
curl -s -X POST https://nile.trongrid.io/wallet/getaccount \
  -d '{"address":"TX7izXWcmofRYonzdcThrS78jifMtVWCuf","visible":true}' \
  > crates/tron/provider/testdata/nile_getaccount.json
curl -s -X POST https://nile.trongrid.io/wallet/gettransactioninfobyid \
  -d '{"value":"3a4d9c5f35b165b1a28f44a57b53cf39b3c2707153e50cdb52d7c0d979750aeb"}' \
  > crates/tron/provider/testdata/nile_txinfo.json
```

Expected: в первом — `"balance"` (наш смоук-аккаунт), во втором — `"blockNumber": 69090417`, `"fee": 1100000` (реальная транзакция смоука плана A).

- [ ] **Step 2: Падающие тесты**

Добавить в `client.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct TxInfo {
    pub block_number: i64,
    pub fee_sun: u64,
    pub energy_used: u64,
    pub success: bool,
    pub contract_address: Option<Address>,
}
```

Тесты (в `mod tests`):

```rust
#[test]
fn parses_account_balance_fixture() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("../testdata/nile_getaccount.json")).unwrap();
    let balance = parse_account_balance(&v).unwrap();
    assert_eq!(balance, v["balance"].as_u64().unwrap());
    assert!(balance > 0);
}

#[test]
fn empty_account_is_zero_balance() {
    assert_eq!(parse_account_balance(&serde_json::json!({})).unwrap(), 0);
}

#[test]
fn parses_tx_info_fixture() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("../testdata/nile_txinfo.json")).unwrap();
    let info = parse_tx_info(&v).unwrap().expect("smoke tx is in a block");
    assert_eq!(info.block_number, 69_090_417);
    assert_eq!(info.fee_sun, 1_100_000);
    assert!(info.success);
    assert_eq!(info.contract_address, None);
}

#[test]
fn pending_tx_info_is_none() {
    assert_eq!(parse_tx_info(&serde_json::json!({})).unwrap(), None);
}

#[tokio::test]
async fn live_balance_and_txinfo() {
    if std::env::var("TRON_LIVE").is_err() {
        eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
        return;
    }
    let p = TronProvider::new("https://nile.trongrid.io").unwrap();
    let addr = foundry_tron_primitives::address::parse("TX7izXWcmofRYonzdcThrS78jifMtVWCuf").unwrap();
    assert!(p.get_balance(addr).await.unwrap() > 0);
    let txid: B256 =
        "3a4d9c5f35b165b1a28f44a57b53cf39b3c2707153e50cdb52d7c0d979750aeb".parse().unwrap();
    let info = p.get_transaction_info(txid).await.unwrap().unwrap();
    assert_eq!(info.block_number, 69_090_417);
}
```

- [ ] **Step 3: Убедиться, что падают** — `cargo test -p foundry-tron-provider` (с PATH-фиксом): FAIL.

- [ ] **Step 4: Реализация**

```rust
pub async fn get_balance(&self, addr: Address) -> Result<u64, TronError> {
    let body = serde_json::json!({ "address": to_hex41(addr), "visible": false });
    let v: serde_json::Value = self.post_json("/wallet/getaccount", body).await?;
    parse_account_balance(&v)
}

pub async fn get_transaction_info(&self, txid: B256) -> Result<Option<TxInfo>, TronError> {
    let body = serde_json::json!({ "value": hex::encode(txid) });
    let v: serde_json::Value = self.post_json("/wallet/gettransactioninfobyid", body).await?;
    parse_tx_info(&v)
}

pub(crate) fn parse_account_balance(v: &serde_json::Value) -> Result<u64, TronError> {
    Ok(v.get("balance").and_then(|b| b.as_u64()).unwrap_or(0))
}

pub(crate) fn parse_tx_info(v: &serde_json::Value) -> Result<Option<TxInfo>, TronError> {
    let Some(block_number) = v.get("blockNumber").and_then(|b| b.as_i64()) else {
        return Ok(None);
    };
    let receipt = &v["receipt"];
    let success = match receipt.get("result").and_then(|r| r.as_str()) {
        None => true, // TransferContract и пр. без VM-исполнения
        Some(r) => r == "SUCCESS",
    };
    let contract_address = match v.get("contract_address").and_then(|c| c.as_str()) {
        Some(h41) => Some(
            foundry_tron_primitives::address::parse(h41)
                .map_err(|e| TronError::Decode(e.to_string()))?,
        ),
        None => None,
    };
    Ok(Some(TxInfo {
        block_number,
        fee_sun: v.get("fee").and_then(|f| f.as_u64()).unwrap_or(0),
        energy_used: receipt.get("energy_usage_total").and_then(|e| e.as_u64()).unwrap_or(0),
        success,
        contract_address,
    }))
}
```

Импорты дополнить: `use alloy_primitives::{Address, B256}; use foundry_tron_primitives::address::to_hex41;`. В `lib.rs` реэкспортнуть `TxInfo`.

- [ ] **Step 5: Тесты зелёные** — `TRON_LIVE=1 cargo test -p foundry-tron-provider`: PASS (8 passed).

- [ ] **Step 6: Commit**

```bash
git add crates/tron/provider
git commit -m "feat(tron): account balance and transaction info queries"
```

---

### Task 3: trigger_constant_contract (чтение контрактов + оценка energy)

**Files:**
- Modify: `crates/tron/provider/src/client.rs`
- Create: `crates/tron/provider/testdata/nile_triggerconstant.json`

**Interfaces:**
- Consumes: `to_hex41`, `Address`
- Produces:
  - `TronProvider::trigger_constant(&self, owner: Address, contract: Address, data: &[u8]) -> Result<ConstantResult, TronError>` — POST `/wallet/triggerconstantcontract` c `{owner_address, contract_address, data: hex(data), visible: false}` (селектор+аргументы уже ABI-закодированы в `data`; кодирование ABI — забота вызывающего, у foundry для этого alloy)
  - `ConstantResult { pub result: Vec<u8>, pub energy_used: u64, pub success: bool }` — `constant_result[0]` из hex, `energy_used`, `result.result == true`
  - `pub(crate) parse_constant_result`

Референс-контракт для fixture: USDT на Nile `TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj`, вызов `totalSupply()` — селектор `18160ddd`.

- [ ] **Step 1: Fixture**

```bash
curl -s -X POST https://nile.trongrid.io/wallet/triggerconstantcontract \
  -d '{"owner_address":"TX7izXWcmofRYonzdcThrS78jifMtVWCuf","contract_address":"TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj","function_selector":"totalSupply()","visible":true}' \
  > crates/tron/provider/testdata/nile_triggerconstant.json
python3 -c "import json;d=json.load(open('crates/tron/provider/testdata/nile_triggerconstant.json'));assert d['result'].get('result')==True, d;print(d['constant_result'][0][:20], d['energy_used'])"
```

Expected: печатает первые hex-цифры totalSupply и energy_used > 0. Если `result.result != true` (адрес не USDT на Nile) — найти актуальный адрес Nile-USDT через nile.tronscan.org и повторить, зафиксировав адрес в комментарии теста.

- [ ] **Step 2: Падающие тесты**

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ConstantResult {
    pub result: Vec<u8>,
    pub energy_used: u64,
    pub success: bool,
}
```

```rust
#[test]
fn parses_constant_result_fixture() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("../testdata/nile_triggerconstant.json")).unwrap();
    let cr = parse_constant_result(&v).unwrap();
    assert!(cr.success);
    assert_eq!(cr.result.len(), 32, "totalSupply returns uint256");
    assert!(cr.energy_used > 0);
    assert_eq!(hex::encode(&cr.result), v["constant_result"][0].as_str().unwrap());
}

#[tokio::test]
async fn live_trigger_constant_total_supply() {
    if std::env::var("TRON_LIVE").is_err() {
        eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
        return;
    }
    let p = TronProvider::new("https://nile.trongrid.io").unwrap();
    let owner = foundry_tron_primitives::address::parse("TX7izXWcmofRYonzdcThrS78jifMtVWCuf").unwrap();
    let usdt = foundry_tron_primitives::address::parse("TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj").unwrap();
    // totalSupply() selector
    let cr = p.trigger_constant(owner, usdt, &hex::decode("18160ddd").unwrap()).await.unwrap();
    assert!(cr.success);
    assert_eq!(cr.result.len(), 32);
    assert!(alloy_primitives::U256::from_be_slice(&cr.result) > alloy_primitives::U256::ZERO);
}
```

- [ ] **Step 3: Убедиться, что падают** — FAIL на `todo!()`/отсутствии функций.

- [ ] **Step 4: Реализация**

```rust
pub async fn trigger_constant(
    &self,
    owner: Address,
    contract: Address,
    data: &[u8],
) -> Result<ConstantResult, TronError> {
    let body = serde_json::json!({
        "owner_address": to_hex41(owner),
        "contract_address": to_hex41(contract),
        "data": hex::encode(data),
        "visible": false,
    });
    let v: serde_json::Value = self.post_json("/wallet/triggerconstantcontract", body).await?;
    parse_constant_result(&v)
}

pub(crate) fn parse_constant_result(v: &serde_json::Value) -> Result<ConstantResult, TronError> {
    let success = v["result"].get("result").and_then(|r| r.as_bool()).unwrap_or(false);
    let result = match v["constant_result"].get(0).and_then(|r| r.as_str()) {
        Some(h) => hex::decode(h).map_err(|e| TronError::Decode(e.to_string()))?,
        None => Vec::new(),
    };
    Ok(ConstantResult {
        result,
        energy_used: v.get("energy_used").and_then(|e| e.as_u64()).unwrap_or(0),
        success,
    })
}
```

Реэкспорт `ConstantResult` в `lib.rs`.

- [ ] **Step 5: Тесты зелёные** — `TRON_LIVE=1 cargo test`: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tron/provider
git commit -m "feat(tron): constant contract calls with energy estimation"
```

---

### Task 4: broadcast + wait_for_confirmation

**Files:**
- Modify: `crates/tron/provider/src/client.rs`

**Interfaces:**
- Consumes: `foundry_tron_primitives::sign::SignedTronTx`, `TxInfo`, `get_transaction_info`
- Produces:
  - `TronProvider::broadcast(&self, tx: &SignedTronTx) -> Result<(), TronError>` — POST `/wallet/broadcasthex` `{"transaction": tx.broadcast_hex()}`; при `result != true` → `TronError::Api { code, message }`, где `message` — hex-декодированное поле `message` ноды (нода отдаёт его в hex)
  - `TronProvider::wait_for_confirmation(&self, txid: B256, max_attempts: u32, interval: Duration) -> Result<TxInfo, TronError>` — поллинг `get_transaction_info` до `Some`, иначе `TronError::Timeout` (лимит обязателен — урок TronBox с бесконечным поллингом)
  - `pub(crate) parse_broadcast_result(v) -> Result<(), TronError>`

- [ ] **Step 1: Падающие тесты**

```rust
#[test]
fn broadcast_success_parses() {
    let v = serde_json::json!({"code": "SUCCESS", "result": true, "txid": "aa"});
    assert!(parse_broadcast_result(&v).is_ok());
}

#[test]
fn broadcast_error_decodes_hex_message() {
    // Реальный формат ошибки ноды: message в hex ("SIGERROR" -> hex ascii)
    let v = serde_json::json!({
        "code": "SIGERROR",
        "message": hex::encode("validate signature error"),
    });
    let err = parse_broadcast_result(&v).unwrap_err();
    match err {
        TronError::Api { code, message } => {
            assert_eq!(code, "SIGERROR");
            assert_eq!(message, "validate signature error");
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn wait_for_confirmation_times_out_on_unknown_tx() {
    if std::env::var("TRON_LIVE").is_err() {
        eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
        return;
    }
    let p = TronProvider::new("https://nile.trongrid.io").unwrap();
    let bogus = B256::repeat_byte(0xab);
    let res = p.wait_for_confirmation(bogus, 2, std::time::Duration::from_millis(300)).await;
    assert!(matches!(res, Err(TronError::Timeout(_))));
}
```

- [ ] **Step 2: Убедиться, что падают** — FAIL.

- [ ] **Step 3: Реализация**

```rust
pub async fn broadcast(&self, tx: &SignedTronTx) -> Result<(), TronError> {
    let body = serde_json::json!({ "transaction": tx.broadcast_hex() });
    let v: serde_json::Value = self.post_json("/wallet/broadcasthex", body).await?;
    parse_broadcast_result(&v)
}

pub async fn wait_for_confirmation(
    &self,
    txid: B256,
    max_attempts: u32,
    interval: std::time::Duration,
) -> Result<TxInfo, TronError> {
    for _ in 0..max_attempts {
        if let Some(info) = self.get_transaction_info(txid).await? {
            return Ok(info);
        }
        tokio::time::sleep(interval).await;
    }
    Err(TronError::Timeout(format!(
        "tx {txid} not confirmed after {max_attempts} attempts"
    )))
}

pub(crate) fn parse_broadcast_result(v: &serde_json::Value) -> Result<(), TronError> {
    if v.get("result").and_then(|r| r.as_bool()) == Some(true) {
        return Ok(());
    }
    let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("UNKNOWN").to_string();
    let raw_msg = v.get("message").and_then(|m| m.as_str()).unwrap_or_default();
    let message = hex::decode(raw_msg)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| raw_msg.to_string());
    Err(TronError::Api { code, message })
}
```

Импорт: `use foundry_tron_primitives::sign::SignedTronTx;`.

- [ ] **Step 4: Тесты зелёные** — PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tron/provider
git commit -m "feat(tron): broadcast and bounded confirmation polling"
```

---

### Task 5: High-level send_transfer + E2E на Nile

**Files:**
- Modify: `crates/tron/provider/src/client.rs`
- Modify: `crates/tron/provider/src/lib.rs`

**Interfaces:**
- Consumes: всё выше + `foundry_tron_primitives::{proto, sign::sign_raw, tapos::RefBlock}`, `alloy_signer_local::PrivateKeySigner`
- Produces:
  - `TronProvider::send_transfer(&self, signer: &PrivateKeySigner, to: Address, amount_sun: i64) -> Result<(B256, TxInfo), TronError>` — полный цикл: `tapos()` → сборка `TransferContract`-raw (expiration = block_ts + 60_000) → `sign_raw` → `broadcast` → `wait_for_confirmation(txid, 20, 3s)`
  - `pub(crate) build_transfer_raw(owner: Address, to: Address, amount_sun: i64, rb: RefBlock, now_ms: i64) -> proto::TransactionRaw` — детерминированная сборка, юнит-тестируемая оффлайн

- [ ] **Step 1: Падающие тесты**

```rust
#[test]
fn builds_transfer_raw_deterministically() {
    use foundry_tron_primitives::tapos::RefBlock;
    let owner = foundry_tron_primitives::address::parse("TX7izXWcmofRYonzdcThrS78jifMtVWCuf").unwrap();
    let to = foundry_tron_primitives::address::parse("T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb").unwrap();
    let rb = RefBlock { bytes: vec![0x3c, 0x6f], hash: vec![1, 2, 3, 4, 5, 6, 7, 8] };
    let raw = build_transfer_raw(owner, to, 1_000_000, rb, 1_783_775_034_896);

    assert_eq!(raw.ref_block_bytes, vec![0x3c, 0x6f]);
    assert_eq!(raw.expiration, 1_783_775_034_896 + 60_000);
    assert_eq!(raw.timestamp, 1_783_775_034_896);
    assert_eq!(raw.contract.len(), 1);
    let c = &raw.contract[0];
    assert_eq!(c.r#type, foundry_tron_primitives::proto::ContractType::TransferContract as i32);
    // parameter декодируется обратно в тот же TransferContract с 21-байтовыми 0x41-адресами
    let tc = <foundry_tron_primitives::proto::TransferContract as prost::Message>::decode(
        c.parameter.as_ref().unwrap().value.as_slice(),
    )
    .unwrap();
    assert_eq!(tc.owner_address[0], 0x41);
    assert_eq!(tc.owner_address[1..], owner.as_slice()[..]);
    assert_eq!(tc.to_address[1..], to.as_slice()[..]);
    assert_eq!(tc.amount, 1_000_000);
}

#[tokio::test]
async fn live_e2e_transfer_on_nile() {
    if std::env::var("TRON_LIVE").is_err() {
        eprintln!("skipped: set TRON_LIVE=1 to run live Nile tests");
        return;
    }
    use std::str::FromStr;
    let key = std::env::var("TRON_PRIVATE_KEY").expect("TRON_PRIVATE_KEY for live E2E");
    let signer = alloy_signer_local::PrivateKeySigner::from_str(&key).unwrap();
    let p = TronProvider::new("https://nile.trongrid.io").unwrap();
    // шлём 0.1 TRX на чёрную дыру (тестнет)
    let to = foundry_tron_primitives::address::parse("T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb").unwrap();
    let (txid, info) = p.send_transfer(&signer, to, 100_000).await.unwrap();
    assert!(info.block_number > 69_000_000);
    assert!(info.success);
    eprintln!("live E2E tx: https://nile.tronscan.org/#/transaction/{}", alloy_primitives::hex::encode(txid));
}
```

В `[dev-dependencies]` уже есть `alloy-signer-local` (Task 1). `prost` нужен в dev-deps для decode в тесте: добавить `prost.workspace = true` в `[dev-dependencies]`.

- [ ] **Step 2: Убедиться, что падают** — FAIL.

- [ ] **Step 3: Реализация**

```rust
use foundry_tron_primitives::{
    proto::{self, ContractType},
    sign::sign_raw,
    tapos::RefBlock,
};
use prost::Message;

pub(crate) fn build_transfer_raw(
    owner: Address,
    to: Address,
    amount_sun: i64,
    rb: RefBlock,
    now_ms: i64,
) -> proto::TransactionRaw {
    let addr21 = |a: Address| {
        let mut v = Vec::with_capacity(21);
        v.push(0x41);
        v.extend_from_slice(a.as_slice());
        v
    };
    let transfer = proto::TransferContract {
        owner_address: addr21(owner),
        to_address: addr21(to),
        amount: amount_sun,
    };
    proto::TransactionRaw {
        ref_block_bytes: rb.bytes,
        ref_block_hash: rb.hash,
        expiration: now_ms + 60_000,
        timestamp: now_ms,
        contract: vec![proto::Contract {
            r#type: ContractType::TransferContract as i32,
            parameter: Some(prost_types::Any {
                type_url: proto::type_url(ContractType::TransferContract).to_string(),
                value: transfer.encode_to_vec(),
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}

impl TronProvider {
    pub async fn send_transfer(
        &self,
        signer: &alloy_signer_local::PrivateKeySigner,
        to: Address,
        amount_sun: i64,
    ) -> Result<(B256, TxInfo), TronError> {
        let (rb, now_ms) = self.tapos().await?;
        let raw = build_transfer_raw(signer.address(), to, amount_sun, rb, now_ms);
        let signed = sign_raw(raw, signer).map_err(|e| TronError::Decode(e.to_string()))?;
        self.broadcast(&signed).await?;
        let info = self
            .wait_for_confirmation(signed.txid, 20, std::time::Duration::from_secs(3))
            .await?;
        Ok((signed.txid, info))
    }
}
```

Зависимости: `alloy-signer-local` и `prost`, `prost-types` переносятся из dev в обычные `[dependencies]` (или добавляются туда — по фактическому использованию).

- [ ] **Step 4: Полный прогон с live E2E**

```bash
source /Users/vaceslaveliseev/@dev/foundry-tron/.env.tron-dev
TC=$(dirname "$(rustup which --toolchain stable cargo)")
PATH="$TC:$PATH" TRON_LIVE=1 TRON_PRIVATE_KEY=$TRON_PRIVATE_KEY cargo test -p foundry-tron-provider
```

Expected: PASS все, live E2E печатает ссылку на реальную транзакцию в nile.tronscan.org.

- [ ] **Step 5: Commit**

```bash
git add crates/tron/provider
git commit -m "feat(tron): high-level send_transfer with live Nile E2E"
```

---

## Критерий завершения плана B

Оффлайн-тесты зелёные без сети; с `TRON_LIVE=1` — все live-тесты зелёные, E2E-перевод виден в Nile explorer. После этого — план C (интеграция `NetworkVariant::Tron` + forge build с tron-solc).
