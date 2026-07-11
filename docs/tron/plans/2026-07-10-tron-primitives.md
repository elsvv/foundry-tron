# tron-primitives Implementation Plan (План A этапа 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Крейт `foundry-tron-primitives` внутри workspace foundry-форка: адресный кодек Tron (base58check/0x41), деривация ключей, protobuf-транзакции с txID = sha256(raw_data), подпись secp256k1 и сборка hex для `/wallet/broadcasthex`.

**Architecture:** Новый изолированный крейт `crates/tron/primitives` без зависимостей на остальной foundry (только alloy-primitives/alloy-signer-local). Внутри ядра адрес — 20-байтовый `alloy_primitives::Address`; префикс `0x41`/base58 появляются только в кодеке. Protobuf — hand-written структуры с prost-derive (без protoc), верифицируемые round-trip-тестом против реальной mainnet-транзакции.

**Tech Stack:** Rust (workspace foundry 1.7.2), prost + prost-types, bs58, sha2, alloy-primitives 1.6, alloy-signer-local (k256 внутри).

**Спека:** `docs/superpowers/specs/2026-07-10-foundry-tron-fork-design.md` (секции 2.4, 2.6, 4.1, 4.2, 4.5, 7).

## Global Constraints

- Все пути ниже — относительно `/Users/vaceslaveliseev/@dev/foundry-tron/foundry/` (это отдельный git-репозиторий; внешний репо проекта не трогаем).
- Работаем в ветке `tron-dev` репозитория foundry.
- Внутреннее представление адреса — всегда 20-байтовый `alloy_primitives::Address`; 21-байтовый `0x41…`/base58check — только на границах I/O (кодек этого крейта).
- 1 TRX = 1_000_000 SUN.
- txID = sha256(protobuf(raw_data)); подпись — secp256k1 по txID, 65 байт `r||s||v`, где `v = 27 + recovery_id` (формат TronWeb/java-tron ECKey).
- Никаких зависимостей на protoc/сборку .proto — prost-derive на hand-written структурах; корректность номеров полей доказывается fixture-тестом (задача 4).
- Тесты: `cargo test -p foundry-tron-primitives` из корня foundry/. Форматирование перед каждым коммитом: `cargo +nightly fmt -- crates/tron/primitives/src/*.rs` (в репо nightly rustfmt; если недоступен — `cargo fmt -p foundry-tron-primitives`).
- Коммиты — conventional commits с префиксом `feat(tron):` / `test(tron):`.

---

### Task 1: Ветка, каркас крейта, модуль units

**Files:**
- Modify: `Cargo.toml` (корень foundry — workspace members + workspace.dependencies)
- Create: `crates/tron/primitives/Cargo.toml`
- Create: `crates/tron/primitives/src/lib.rs`
- Create: `crates/tron/primitives/src/units.rs`

**Interfaces:**
- Produces: пустой компилирующийся крейт `foundry-tron-primitives`; `units::SUN_PER_TRX: u64`, `units::trx_to_sun(u64) -> u64`, `units::format_sun_as_trx(u64) -> String`.

- [ ] **Step 1: Создать ветку**

```bash
cd /Users/vaceslaveliseev/@dev/foundry-tron/foundry
git checkout -b tron-dev
```

- [ ] **Step 2: Изучить workspace-конвенции**

```bash
grep -n "^members\|^\[workspace\]" -A 40 Cargo.toml | head -60
grep -n "sha2\|thiserror\|serde_json" Cargo.toml
ls crates/anvil/core  # пример вложенного member
```

Зафиксировать: (а) members — явный список или glob; (б) какие из sha2/thiserror/serde_json уже есть в `[workspace.dependencies]`.

- [ ] **Step 3: Добавить крейт в workspace**

В корневом `Cargo.toml`: если members — явный список, добавить `"crates/tron/primitives"` (по алфавиту рядом с другими crates/*). Если glob `"crates/*"` — вложенный путь всё равно добавить явно (glob не рекурсивный). В `[workspace.dependencies]` добавить отсутствующие:

```toml
# только те, которых нет:
bs58 = "0.5"
prost = "0.13"
prost-types = "0.13"
sha2 = "0.10"
```

- [ ] **Step 4: Cargo.toml крейта**

`crates/tron/primitives/Cargo.toml` (поля package скопировать по образцу `crates/primitives/Cargo.toml` — version/edition/rust-version/license/repository через `.workspace = true`):

```toml
[package]
name = "foundry-tron-primitives"
description = "Tron primitives: address codec, protobuf transactions, signing"
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
alloy-primitives.workspace = true
alloy-signer.workspace = true
alloy-signer-local.workspace = true
bs58.workspace = true
prost.workspace = true
prost-types.workspace = true
sha2.workspace = true
thiserror.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

- [ ] **Step 5: Написать падающий тест units**

`crates/tron/primitives/src/units.rs`:

```rust
//! TRX/SUN unit conversions. 1 TRX = 1_000_000 SUN.

pub const SUN_PER_TRX: u64 = 1_000_000;

pub fn trx_to_sun(trx: u64) -> u64 {
    trx * SUN_PER_TRX
}

/// Formats a SUN amount as a decimal TRX string, e.g. 1_500_000 -> "1.500000".
pub fn format_sun_as_trx(sun: u64) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_trx_to_sun() {
        assert_eq!(trx_to_sun(1), 1_000_000);
        assert_eq!(trx_to_sun(1000), 1_000_000_000);
    }

    #[test]
    fn formats_sun_as_trx() {
        assert_eq!(format_sun_as_trx(1_500_000), "1.500000");
        assert_eq!(format_sun_as_trx(0), "0.000000");
        assert_eq!(format_sun_as_trx(1), "0.000001");
    }
}
```

`crates/tron/primitives/src/lib.rs`:

```rust
//! # foundry-tron-primitives
//!
//! Tron protocol primitives: address codec (base58check / 0x41-hex),
//! protobuf transactions (txID = sha256(raw_data)), secp256k1 signing.

pub mod units;
```

- [ ] **Step 6: Убедиться, что тест падает**

Run: `cargo test -p foundry-tron-primitives`
Expected: FAIL — `formats_sun_as_trx` паникует на `todo!()`; `converts_trx_to_sun` проходит.

- [ ] **Step 7: Минимальная реализация**

```rust
pub fn format_sun_as_trx(sun: u64) -> String {
    format!("{}.{:06}", sun / SUN_PER_TRX, sun % SUN_PER_TRX)
}
```

- [ ] **Step 8: Тесты зелёные**

Run: `cargo test -p foundry-tron-primitives`
Expected: PASS (2 passed).

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/tron/primitives
git commit -m "feat(tron): scaffold foundry-tron-primitives crate with TRX/SUN units"
```

---

### Task 2: Адресный кодек (base58check / 0x41-hex / 0x-hex)

**Files:**
- Create: `crates/tron/primitives/src/address.rs`
- Modify: `crates/tron/primitives/src/lib.rs` (добавить `pub mod address;`)

**Interfaces:**
- Consumes: —
- Produces:
  - `address::TRON_ADDRESS_PREFIX: u8` (= 0x41)
  - `address::to_base58(addr: Address) -> String`
  - `address::to_hex41(addr: Address) -> String` — `"41" + 40 hex-символов` без 0x
  - `address::parse(s: &str) -> Result<Address, AddressError>` — принимает `T…` base58check, `41…` (42 hex-символа), `0x…` (20 байт)
  - `address::AddressError` (thiserror enum: `InvalidBase58`, `InvalidChecksum`, `InvalidPrefix`, `InvalidLength`, `InvalidHex`)

Тестовые векторы (реальные, mainnet):
- «Чёрная дыра»: hex `410000000000000000000000000000000000000000` ↔ `T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb`
- Контракт USDT: hex `41a614f803b6fd780986a42c78ec9c7f77e6ded13c` ↔ `TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t`

- [ ] **Step 1: Написать падающие тесты**

`crates/tron/primitives/src/address.rs`:

```rust
//! Tron address codec.
//!
//! Internally addresses are 20-byte [`Address`]; the 0x41 prefix and
//! base58check exist only at I/O boundaries.

use alloy_primitives::{hex, Address};
use sha2::{Digest, Sha256};

pub const TRON_ADDRESS_PREFIX: u8 = 0x41;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AddressError {
    #[error("invalid base58: {0}")]
    InvalidBase58(String),
    #[error("invalid base58check checksum")]
    InvalidChecksum,
    #[error("expected 0x41 address prefix, got {0:#x}")]
    InvalidPrefix(u8),
    #[error("invalid address length: {0}")]
    InvalidLength(usize),
    #[error("invalid hex: {0}")]
    InvalidHex(String),
}

pub fn to_base58(addr: Address) -> String {
    todo!()
}

pub fn to_hex41(addr: Address) -> String {
    todo!()
}

pub fn parse(s: &str) -> Result<Address, AddressError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    const USDT_HEX20: Address = address!("a614f803b6fd780986a42c78ec9c7f77e6ded13c");
    const USDT_BASE58: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
    const BLACKHOLE_BASE58: &str = "T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb";

    #[test]
    fn encodes_base58() {
        assert_eq!(to_base58(USDT_HEX20), USDT_BASE58);
        assert_eq!(to_base58(Address::ZERO), BLACKHOLE_BASE58);
    }

    #[test]
    fn encodes_hex41() {
        assert_eq!(to_hex41(USDT_HEX20), "41a614f803b6fd780986a42c78ec9c7f77e6ded13c");
    }

    #[test]
    fn parses_all_three_formats() {
        assert_eq!(parse(USDT_BASE58).unwrap(), USDT_HEX20);
        assert_eq!(parse("41a614f803b6fd780986a42c78ec9c7f77e6ded13c").unwrap(), USDT_HEX20);
        assert_eq!(parse("0xa614f803b6fd780986a42c78ec9c7f77e6ded13c").unwrap(), USDT_HEX20);
    }

    #[test]
    fn roundtrips() {
        let addr = address!("1234567890abcdef1234567890abcdef12345678");
        assert_eq!(parse(&to_base58(addr)).unwrap(), addr);
        assert_eq!(parse(&to_hex41(addr)).unwrap(), addr);
    }

    #[test]
    fn rejects_bad_checksum() {
        // последний символ изменён
        let bad = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6u";
        assert_eq!(parse(bad).unwrap_err(), AddressError::InvalidChecksum);
    }

    #[test]
    fn rejects_wrong_prefix_and_length() {
        assert!(matches!(parse("4200000000000000000000000000000000000000ff"), Err(AddressError::InvalidPrefix(0x42))));
        assert!(matches!(parse("0x1234"), Err(AddressError::InvalidHex(_)) | Err(AddressError::InvalidLength(_))));
    }
}
```

В `lib.rs` добавить `pub mod address;`.

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test -p foundry-tron-primitives address`
Expected: FAIL — паники `todo!()`.

- [ ] **Step 3: Реализация**

```rust
fn checksum(payload: &[u8]) -> [u8; 4] {
    let d = Sha256::digest(Sha256::digest(payload));
    [d[0], d[1], d[2], d[3]]
}

pub fn to_base58(addr: Address) -> String {
    let mut payload = Vec::with_capacity(25);
    payload.push(TRON_ADDRESS_PREFIX);
    payload.extend_from_slice(addr.as_slice());
    let ck = checksum(&payload);
    payload.extend_from_slice(&ck);
    bs58::encode(payload).into_string()
}

pub fn to_hex41(addr: Address) -> String {
    format!("41{}", hex::encode(addr.as_slice()))
}

pub fn parse(s: &str) -> Result<Address, AddressError> {
    if let Some(h) = s.strip_prefix("0x") {
        let bytes = hex::decode(h).map_err(|e| AddressError::InvalidHex(e.to_string()))?;
        if bytes.len() != 20 {
            return Err(AddressError::InvalidLength(bytes.len()));
        }
        return Ok(Address::from_slice(&bytes));
    }
    if s.len() == 42 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes = hex::decode(s).map_err(|e| AddressError::InvalidHex(e.to_string()))?;
        if bytes[0] != TRON_ADDRESS_PREFIX {
            return Err(AddressError::InvalidPrefix(bytes[0]));
        }
        return Ok(Address::from_slice(&bytes[1..]));
    }
    // base58check
    let decoded = bs58::decode(s)
        .into_vec()
        .map_err(|e| AddressError::InvalidBase58(e.to_string()))?;
    if decoded.len() != 25 {
        return Err(AddressError::InvalidLength(decoded.len()));
    }
    let (payload, ck) = decoded.split_at(21);
    if checksum(payload) != ck {
        return Err(AddressError::InvalidChecksum);
    }
    if payload[0] != TRON_ADDRESS_PREFIX {
        return Err(AddressError::InvalidPrefix(payload[0]));
    }
    Ok(Address::from_slice(&payload[1..]))
}
```

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p foundry-tron-primitives address`
Expected: PASS (6 passed). Если `encodes_base58` падает на векторах — проверить порядок: checksum считается от 21-байтового payload (0x41+addr), первые 4 байта двойного sha256.

- [ ] **Step 5: Commit**

```bash
git add crates/tron/primitives
git commit -m "feat(tron): base58check/hex41 address codec with mainnet vectors"
```

---

### Task 3: Деривация ключ → адрес

**Files:**
- Create: `crates/tron/primitives/src/keys.rs`
- Modify: `crates/tron/primitives/src/lib.rs` (добавить `pub mod keys;`)

**Interfaces:**
- Consumes: `address::to_base58`
- Produces: `keys::signer_base58_address(signer: &PrivateKeySigner) -> String`. Деривация Tron идентична Ethereum (keccak256(pubkey)[12..]) — переиспользуем `alloy_signer_local::PrivateKeySigner::address()`, свой крипто-код не пишем.

- [ ] **Step 1: Написать падающий тест**

`crates/tron/primitives/src/keys.rs`:

```rust
//! Key-to-address helpers. Tron derives the 20-byte body exactly like
//! Ethereum (keccak256(uncompressed_pubkey)[12..]); only the display
//! encoding (0x41 + base58check) differs.

use crate::address;
use alloy_signer_local::PrivateKeySigner;

pub fn signer_base58_address(signer: &PrivateKeySigner) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // Известный Ethereum-вектор: тело адреса у Tron то же самое.
    const PRIVKEY: &str = "4c0883a69102937d6231471b5dbb6204fe512961708279feb1be6ae5538da033";
    const ETH_ADDR: &str = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23";

    #[test]
    fn derives_same_20_bytes_as_ethereum() {
        let signer = PrivateKeySigner::from_str(PRIVKEY).unwrap();
        assert_eq!(signer.address().to_checksum(None), ETH_ADDR);
    }

    #[test]
    fn formats_signer_address_as_base58() {
        let signer = PrivateKeySigner::from_str(PRIVKEY).unwrap();
        let t = signer_base58_address(&signer);
        assert!(t.starts_with('T'));
        assert_eq!(address::parse(&t).unwrap(), signer.address());
    }
}
```

В `lib.rs` добавить `pub mod keys;`.

- [ ] **Step 2: Убедиться, что тест падает**

Run: `cargo test -p foundry-tron-primitives keys`
Expected: FAIL — `todo!()` во втором тесте; первый (чистый alloy) должен пройти. Если первый падает — вектор/импорт неверны, разобраться до продолжения.

- [ ] **Step 3: Реализация**

```rust
pub fn signer_base58_address(signer: &PrivateKeySigner) -> String {
    address::to_base58(signer.address())
}
```

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p foundry-tron-primitives keys`
Expected: PASS (2 passed).

- [ ] **Step 5: Commit**

```bash
git add crates/tron/primitives
git commit -m "feat(tron): key-to-address derivation via alloy signer"
```

---

### Task 4: Protobuf-типы транзакций + fixture с mainnet + txID

**Files:**
- Create: `crates/tron/primitives/src/proto.rs`
- Create: `crates/tron/primitives/testdata/mainnet_trigger_tx.json`
- Modify: `crates/tron/primitives/src/lib.rs` (добавить `pub mod proto;`)

**Interfaces:**
- Consumes: —
- Produces (все типы derive `Clone, PartialEq, prost::Message`):
  - `proto::TransactionRaw` — поля: `ref_block_bytes: Vec<u8>` (tag 1), `ref_block_num: i64` (tag 3), `ref_block_hash: Vec<u8>` (tag 4), `expiration: i64` (tag 8), `data: Vec<u8>` (tag 10), `contract: Vec<Contract>` (tag 11), `scripts: Vec<u8>` (tag 12), `timestamp: i64` (tag 14), `fee_limit: i64` (tag 18)
  - `proto::Contract` — `r#type: i32` (tag 1), `parameter: Option<prost_types::Any>` (tag 2), `provider: Vec<u8>` (tag 3), `contract_name: Vec<u8>` (tag 4), `permission_id: i32` (tag 5)
  - `proto::Transaction` — `raw_data: Option<TransactionRaw>` (tag 1), `signature: Vec<Vec<u8>>` (tag 2)
  - `proto::TriggerSmartContract` — `owner_address: Vec<u8>` (tag 1), `contract_address: Vec<u8>` (tag 2), `call_value: i64` (tag 3), `data: Vec<u8>` (tag 4), `call_token_value: i64` (tag 5), `token_id: i64` (tag 6)
  - `proto::TransferContract` — `owner_address: Vec<u8>` (tag 1), `to_address: Vec<u8>` (tag 2), `amount: i64` (tag 3)
  - `proto::CreateSmartContract` — `owner_address: Vec<u8>` (tag 1), `new_contract: Option<SmartContract>` (tag 2), `call_token_value: i64` (tag 3), `token_id: i64` (tag 4)
  - `proto::SmartContract` — `origin_address: Vec<u8>` (tag 1), `contract_address: Vec<u8>` (tag 2), `bytecode: Vec<u8>` (tag 4), `call_value: i64` (tag 5), `consume_user_resource_percent: i64` (tag 6), `name: String` (tag 7), `origin_energy_limit: i64` (tag 8) — поле `abi` (tag 3) сознательно опускаем (деплой без он-чейн ABI, как делают tronweb с `abi: []`); если fixture-тест CreateSmartContract потребует — добавить.
  - константы: `proto::ContractType::{TransferContract = 1, CreateSmartContract = 30, TriggerSmartContract = 31}` (enum as i32), `proto::type_url(ct: ContractType) -> &'static str` (`"type.googleapis.com/protocol.TriggerSmartContract"` и т.д.)
  - `proto::txid(raw: &TransactionRaw) -> B256` — sha256 от `raw.encode_to_vec()`

- [ ] **Step 1: Сверить номера полей с официальным .proto**

```bash
curl -s https://raw.githubusercontent.com/tronprotocol/protocol/master/core/Tron.proto | sed -n '/message Transaction {/,/^}/p'
curl -s https://raw.githubusercontent.com/tronprotocol/protocol/master/core/contract/smart_contract.proto | sed -n '/message TriggerSmartContract/,/^}/p;/message CreateSmartContract/,/^}/p;/message SmartContract/,/^}/p'
curl -s https://raw.githubusercontent.com/tronprotocol/protocol/master/core/contract/balance_contract.proto | sed -n '/message TransferContract/,/^}/p'
```

Сверить каждый tag из Interfaces выше с выводом. При расхождении — исправить Interfaces/код (истина — официальный .proto). Внимание на `message raw` внутри `Transaction` (это и есть TransactionRaw) и `message Contract` внутри `Transaction`.

- [ ] **Step 2: Скачать fixture с mainnet**

```bash
mkdir -p crates/tron/primitives/testdata
curl -s -X POST https://api.trongrid.io/wallet/getnowblock \
  | jq '[.transactions[] | select(.raw_data.contract[0].type == "TriggerSmartContract" and (.raw_data.contract[0].Permission_id == null))][0] | {txID, raw_data_hex}' \
  > crates/tron/primitives/testdata/mainnet_trigger_tx.json
cat crates/tron/primitives/testdata/mainnet_trigger_tx.json
```

Expected: JSON с непустыми `txID` (64 hex) и `raw_data_hex`. Если `null` — повторить (в блоке не оказалось подходящей транзакции).

- [ ] **Step 3: Написать падающий тест (round-trip + txID)**

`crates/tron/primitives/src/proto.rs` — структуры из Interfaces с prost-атрибутами. Образец (остальные по аналогии, у ВСЕХ типов — точные tag из Step 1):

```rust
//! Hand-written prost mappings of the Tron protocol messages we need.
//! Field numbers mirror tronprotocol/protocol; correctness is proven by
//! the mainnet fixture round-trip test below.

use alloy_primitives::B256;
use prost::Message;
use sha2::{Digest, Sha256};

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TransactionRaw {
    #[prost(bytes = "vec", tag = "1")]
    pub ref_block_bytes: Vec<u8>,
    #[prost(int64, tag = "3")]
    pub ref_block_num: i64,
    #[prost(bytes = "vec", tag = "4")]
    pub ref_block_hash: Vec<u8>,
    #[prost(int64, tag = "8")]
    pub expiration: i64,
    #[prost(bytes = "vec", tag = "10")]
    pub data: Vec<u8>,
    #[prost(message, repeated, tag = "11")]
    pub contract: Vec<Contract>,
    #[prost(bytes = "vec", tag = "12")]
    pub scripts: Vec<u8>,
    #[prost(int64, tag = "14")]
    pub timestamp: i64,
    #[prost(int64, tag = "18")]
    pub fee_limit: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ContractType {
    TransferContract = 1,
    CreateSmartContract = 30,
    TriggerSmartContract = 31,
}

pub fn type_url(ct: ContractType) -> &'static str {
    match ct {
        ContractType::TransferContract => "type.googleapis.com/protocol.TransferContract",
        ContractType::CreateSmartContract => "type.googleapis.com/protocol.CreateSmartContract",
        ContractType::TriggerSmartContract => "type.googleapis.com/protocol.TriggerSmartContract",
    }
}

pub fn txid(raw: &TransactionRaw) -> B256 {
    B256::from_slice(&Sha256::digest(raw.encode_to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    fn fixture() -> (Vec<u8>, String) {
        let json: serde_json::Value = serde_json::from_str(include_str!(
            "../testdata/mainnet_trigger_tx.json"
        ))
        .unwrap();
        let raw = hex::decode(json["raw_data_hex"].as_str().unwrap()).unwrap();
        let txid = json["txID"].as_str().unwrap().to_string();
        (raw, txid)
    }

    #[test]
    fn fixture_roundtrip_is_byte_identical() {
        let (bytes, _) = fixture();
        let raw = TransactionRaw::decode(bytes.as_slice()).unwrap();
        assert!(!raw.contract.is_empty());
        assert_eq!(raw.encode_to_vec(), bytes, "re-encode must be byte-identical");
    }

    #[test]
    fn txid_matches_mainnet() {
        let (bytes, expected) = fixture();
        let raw = TransactionRaw::decode(bytes.as_slice()).unwrap();
        assert_eq!(hex::encode(txid(&raw)), expected);
    }

    #[test]
    fn trigger_contract_any_roundtrip() {
        let trig = TriggerSmartContract {
            owner_address: vec![0x41; 21],
            contract_address: vec![0x41; 21],
            call_value: 5,
            data: vec![0xde, 0xad],
            call_token_value: 0,
            token_id: 0,
        };
        let any = prost_types::Any {
            type_url: type_url(ContractType::TriggerSmartContract).to_string(),
            value: trig.encode_to_vec(),
        };
        let back = TriggerSmartContract::decode(any.value.as_slice()).unwrap();
        assert_eq!(back, trig);
    }
}
```

В `lib.rs` добавить `pub mod proto;`.

- [ ] **Step 4: Убедиться, что компилируется и тесты гоняются**

Run: `cargo test -p foundry-tron-primitives proto`
Expected: до реализации всех структур — ошибки компиляции; после набивки структур тесты должны пройти сразу. Если `fixture_roundtrip_is_byte_identical` падает — в raw_data есть поле, не описанное в структуре (prost молча съел unknown field). Диагностика: `protoc --decode_raw` недоступен без protoc, поэтому смотреть теги вручную: `python3 -c "..."` не нужен — проще добавить недостающее поле по официальному .proto из Step 1 (кандидаты: `auths` tag 9). Если падает именно на auths — добавить структуру `Authority`/`AccountId` по .proto.

- [ ] **Step 5: Тесты зелёные**

Run: `cargo test -p foundry-tron-primitives proto`
Expected: PASS (3 passed) — это доказывает корректность номеров полей и кодирования против реального mainnet.

- [ ] **Step 6: Commit**

```bash
git add crates/tron/primitives
git commit -m "feat(tron): protobuf tx types with mainnet fixture round-trip + txid"
```

---

### Task 5: TAPOS-хелперы

**Files:**
- Create: `crates/tron/primitives/src/tapos.rs`
- Modify: `crates/tron/primitives/src/lib.rs` (добавить `pub mod tapos;`)

**Interfaces:**
- Consumes: —
- Produces: `tapos::RefBlock { pub bytes: Vec<u8>, pub hash: Vec<u8> }`, `tapos::ref_block(block_number: i64, block_id: &[u8; 32]) -> RefBlock`. Правило Tron: `ref_block_bytes` = байты 6..8 big-endian представления номера блока (2 байта), `ref_block_hash` = байты 8..16 блок-ID.

- [ ] **Step 1: Написать падающий тест**

```rust
//! TAPOS (Transaction as Proof of Stake) reference-block fields.

pub struct RefBlock {
    pub bytes: Vec<u8>,
    pub hash: Vec<u8>,
}

pub fn ref_block(block_number: i64, block_id: &[u8; 32]) -> RefBlock {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_ref_block_fields() {
        // block 0x0102030405060708 -> big-endian bytes [01,02,03,04,05,06,07,08], take [6..8]
        let mut id = [0u8; 32];
        for (i, b) in id.iter_mut().enumerate() {
            *b = i as u8;
        }
        let rb = ref_block(0x0102030405060708, &id);
        assert_eq!(rb.bytes, vec![0x07, 0x08]);
        assert_eq!(rb.hash, vec![8, 9, 10, 11, 12, 13, 14, 15]);
    }

    #[test]
    fn real_block_number() {
        // блок 63156856 = 0x03C39A78 -> [..,0x9A,0x78]
        let rb = ref_block(63_156_856, &[0u8; 32]);
        assert_eq!(rb.bytes, vec![0x9a, 0x78]);
    }
}
```

- [ ] **Step 2: Убедиться, что тест падает**

Run: `cargo test -p foundry-tron-primitives tapos`
Expected: FAIL (`todo!()`).

- [ ] **Step 3: Реализация**

```rust
pub fn ref_block(block_number: i64, block_id: &[u8; 32]) -> RefBlock {
    let be = block_number.to_be_bytes();
    RefBlock { bytes: be[6..8].to_vec(), hash: block_id[8..16].to_vec() }
}
```

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p foundry-tron-primitives tapos`
Expected: PASS (2 passed).

- [ ] **Step 5: Commit**

```bash
git add crates/tron/primitives
git commit -m "feat(tron): TAPOS ref-block helpers"
```

---

### Task 6: Подпись и сборка broadcast-hex

**Files:**
- Create: `crates/tron/primitives/src/sign.rs`
- Modify: `crates/tron/primitives/src/lib.rs` (добавить `pub mod sign;` и реэкспорты — см. Step 5)

**Interfaces:**
- Consumes: `proto::{Transaction, TransactionRaw, txid}`
- Produces:
  - `sign::SignedTronTx { pub txid: B256, pub tx: proto::Transaction }`
  - `sign::sign_raw(raw: proto::TransactionRaw, signer: &PrivateKeySigner) -> alloy_signer::Result<SignedTronTx>` — подпись 65 байт `r||s||(27 + recovery_id)` по txid
  - `SignedTronTx::broadcast_hex(&self) -> String` — hex protobuf полного `Transaction` для `POST /wallet/broadcasthex`

- [ ] **Step 1: Написать падающий тест**

`crates/tron/primitives/src/sign.rs`:

```rust
//! Transaction signing: secp256k1 over txid (= sha256(raw_data)),
//! 65-byte r||s||v signature with v = 27 + recovery_id (TronWeb format).

use crate::proto::{self, Transaction, TransactionRaw};
use alloy_primitives::{hex, B256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use prost::Message;

pub struct SignedTronTx {
    pub txid: B256,
    pub tx: Transaction,
}

impl SignedTronTx {
    pub fn broadcast_hex(&self) -> String {
        todo!()
    }
}

pub fn sign_raw(
    raw: TransactionRaw,
    signer: &PrivateKeySigner,
) -> alloy_signer::Result<SignedTronTx> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn sample_raw() -> TransactionRaw {
        TransactionRaw {
            ref_block_bytes: vec![0x9a, 0x78],
            ref_block_hash: vec![1, 2, 3, 4, 5, 6, 7, 8],
            expiration: 1_700_000_060_000,
            timestamp: 1_700_000_000_000,
            fee_limit: 1_000_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn signature_is_65_bytes_and_recovers_to_signer() {
        let signer = PrivateKeySigner::from_str(
            "4c0883a69102937d6231471b5dbb6204fe512961708279feb1be6ae5538da033",
        )
        .unwrap();
        let signed = sign_raw(sample_raw(), &signer).unwrap();

        let sig = &signed.tx.signature[0];
        assert_eq!(sig.len(), 65);
        assert!(sig[64] == 27 || sig[64] == 28, "v must be 27 + recid");

        // подпись должна восстанавливаться в адрес подписанта
        let rs = alloy_primitives::Signature::from_raw(sig).unwrap();
        let recovered = rs.recover_address_from_prehash(&signed.txid).unwrap();
        assert_eq!(recovered, signer.address());
    }

    #[test]
    fn txid_is_sha256_of_raw() {
        let signer = PrivateKeySigner::from_str(
            "4c0883a69102937d6231471b5dbb6204fe512961708279feb1be6ae5538da033",
        )
        .unwrap();
        let raw = sample_raw();
        let expected = proto::txid(&raw);
        let signed = sign_raw(raw, &signer).unwrap();
        assert_eq!(signed.txid, expected);
    }

    #[test]
    fn broadcast_hex_decodes_back() {
        let signer = PrivateKeySigner::from_str(
            "4c0883a69102937d6231471b5dbb6204fe512961708279feb1be6ae5538da033",
        )
        .unwrap();
        let signed = sign_raw(sample_raw(), &signer).unwrap();
        let bytes = hex::decode(signed.broadcast_hex()).unwrap();
        let decoded = Transaction::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded, signed.tx);
        assert_eq!(decoded.signature.len(), 1);
    }
}
```

Примечание: если `alloy_primitives::Signature::from_raw` ожидает v=0/1 и вернёт ошибку на 27/28 — использовать `Signature::from_bytes_and_parity(&sig[..64], sig[64] >= 28)`; проверить актуальную сигнатуру API alloy-primitives 1.6 по `cargo doc` или исходникам в `~/.cargo`.

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test -p foundry-tron-primitives sign`
Expected: FAIL (`todo!()`).

- [ ] **Step 3: Реализация**

```rust
impl SignedTronTx {
    pub fn broadcast_hex(&self) -> String {
        hex::encode(self.tx.encode_to_vec())
    }
}

pub fn sign_raw(
    raw: TransactionRaw,
    signer: &PrivateKeySigner,
) -> alloy_signer::Result<SignedTronTx> {
    let txid = proto::txid(&raw);
    let sig = signer.sign_hash_sync(&txid)?;
    let mut bytes = Vec::with_capacity(65);
    bytes.extend_from_slice(&sig.r().to_be_bytes::<32>());
    bytes.extend_from_slice(&sig.s().to_be_bytes::<32>());
    bytes.push(27 + sig.v() as u8);
    Ok(SignedTronTx { txid, tx: Transaction { raw_data: Some(raw), signature: vec![bytes] } })
}
```

(`sig.v()` в alloy 1.6 возвращает parity как bool → `as u8` даёт 0/1; если API отличается — взять `sig.recid().to_byte()`.)

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p foundry-tron-primitives sign`
Expected: PASS (3 passed).

- [ ] **Step 5: Реэкспорты + полный прогон**

В `lib.rs`:

```rust
pub use address::{parse as parse_address, to_base58, to_hex41};
pub use proto::{txid, ContractType, Transaction, TransactionRaw, TriggerSmartContract};
pub use sign::{sign_raw, SignedTronTx};
pub use tapos::ref_block;
```

Run: `cargo test -p foundry-tron-primitives`
Expected: PASS — все тесты крейта.

- [ ] **Step 6: Commit**

```bash
git add crates/tron/primitives
git commit -m "feat(tron): secp256k1 tx signing and broadcast-hex assembly"
```

---

### Task 7: Смоук против реальной сети Nile (ручная проверка, gate плана)

**Files:**
- Create: `crates/tron/primitives/examples/nile_smoke.rs`

**Interfaces:**
- Consumes: всё из задач 2–6.
- Produces: доказательство, что наши protobuf+подпись принимает реальная нода (это gate перед планом B). Не юнит-тест (сеть!), а example-бинарь.

- [ ] **Step 1: Получить тестовый аккаунт Nile**

Сгенерировать ключ и адрес:

```bash
cargo run -p foundry-tron-primitives --example nile_smoke -- address
```

(первая подкоманда example — см. Step 2). Пополнить адрес (это ключ-owner) через кран https://nileex.io/join/getJoinPage (ручной шаг — попросить пользователя, если CAPTCHA). Отдельно получателя для `send` создать через `nile_smoke -- keygen` (см. Step 3) — он должен отличаться от owner.

- [ ] **Step 2: Написать example**

`crates/tron/primitives/examples/nile_smoke.rs`:

```rust
//! Manual smoke test against Nile testnet.
//!
//! Usage:
//!   cargo run --example nile_smoke -- keygen            # throwaway recipient key + address
//!   cargo run --example nile_smoke -- address           # print funded-key address for faucet
//!   cargo run --example nile_smoke -- send <to_T_addr>  # transfer 1 TRX to a DIFFERENT address
//!
//! `address`/`send` require TRON_PRIVATE_KEY env var (hex, no 0x).
//! NB: java-tron rejects owner == to ("Cannot transfer TRX to yourself"), so
//! <to_T_addr> must differ from the funded key — use `keygen` for a throwaway one.

use foundry_tron_primitives::{proto, sign, tapos, to_base58};
use alloy_signer_local::PrivateKeySigner;
use prost::Message;
use std::str::FromStr;

const NILE: &str = "https://nile.trongrid.io";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let signer = PrivateKeySigner::from_str(&std::env::var("TRON_PRIVATE_KEY")?)?;

    match args[1].as_str() {
        "address" => {
            println!("{}", to_base58(signer.address()));
        }
        "send" => {
            let to = foundry_tron_primitives::parse_address(&args[2])?;
            // 1. TAPOS из свежего блока
            let block: serde_json::Value =
                ureq::post(&format!("{NILE}/wallet/getnowblock")).call()?.into_json()?;
            let number = block["block_header"]["raw_data"]["number"].as_i64().unwrap();
            let block_id: [u8; 32] = alloy_primitives::hex::decode(
                block["blockID"].as_str().unwrap(),
            )?
            .try_into()
            .unwrap();
            let rb = tapos::ref_block(number, &block_id);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as i64;

            // 2. TransferContract 1 TRX
            let mut owner21 = vec![0x41];
            owner21.extend_from_slice(signer.address().as_slice());
            let mut to21 = vec![0x41];
            to21.extend_from_slice(to.as_slice());
            let transfer = proto::TransferContract {
                owner_address: owner21,
                to_address: to21,
                amount: 1_000_000,
            };
            let raw = proto::TransactionRaw {
                ref_block_bytes: rb.bytes,
                ref_block_hash: rb.hash,
                expiration: now_ms + 60_000,
                timestamp: now_ms,
                contract: vec![proto::Contract {
                    r#type: proto::ContractType::TransferContract as i32,
                    parameter: Some(prost_types::Any {
                        type_url: proto::type_url(proto::ContractType::TransferContract)
                            .to_string(),
                        value: transfer.encode_to_vec(),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            };

            // 3. Подпись и бродкаст
            let signed = sign::sign_raw(raw, &signer)?;
            println!("txid: {}", signed.txid);
            let resp: serde_json::Value = ureq::post(&format!("{NILE}/wallet/broadcasthex"))
                .send_json(serde_json::json!({ "transaction": signed.broadcast_hex() }))?
                .into_json()?;
            println!("broadcast response: {resp}");
        }
        _ => eprintln!("usage: nile_smoke address | send <to>"),
    }
    Ok(())
}
```

В `[dev-dependencies]` крейта добавить `ureq = { version = "2", features = ["json"] }` (только dev — в библиотеку HTTP не тянем, это работа плана B).

- [ ] **Step 3: Прогнать смоук**

```bash
export TRON_PRIVATE_KEY=<ключ из Step 1>
# получить ДРУГОЙ адрес-получатель (java-tron запрещает перевод самому себе):
cargo run -p foundry-tron-primitives --example nile_smoke -- keygen
cargo run -p foundry-tron-primitives --example nile_smoke -- send <T-адрес из keygen>
```

Получатель должен отличаться от адреса ключа: TransferActuator.validate() в java-tron отклоняет `owner == to` («Cannot transfer TRX to yourself»). `keygen` печатает свежий throwaway-ключ и его T-адрес; финансировать получателя не нужно, чтобы принять TRX. (В сам example встроен guard: `send` на свой же адрес падает локально до бродкаста.)

Expected: `broadcast response: {"result":true,"txid":"..."}`, txid совпадает с нашим. Проверить в explorer: `https://nile.tronscan.org/#/transaction/<txid>`.
Если `result:false` c `SIGERROR` — неверен формат v (попробовать v=recid без +27 — и зафиксировать результат в комментарии `sign.rs`); с `TAPOS_ERROR` — проверить ref_block; c `TRANSACTION_EXPIRATION_ERROR` — увеличить expiration; c `CONTRACT_VALIDATE_ERROR` «Cannot transfer TRX to yourself» — получатель совпал с owner, взять другой адрес (см. `keygen`).

- [ ] **Step 4: Commit**

```bash
git add crates/tron/primitives
git commit -m "feat(tron): nile smoke example proving protobuf+signing against real node"
```

---

## Критерий завершения плана A

`cargo test -p foundry-tron-primitives` зелёный, смоук на Nile прошёл (транзакция видна в explorer). После этого пишутся планы B (tron-provider поверх этих примитивов), C (NetworkVariant::Tron + build), D (cast/script).
