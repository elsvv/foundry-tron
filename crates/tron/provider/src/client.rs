//! Typed async client for the Tron wallet HTTP API.

use alloy_primitives::hex;
use foundry_tron_primitives::tapos::{RefBlock, ref_block};
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
        let mut req = self.client.post(format!("{}{path}", self.base_url)).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("TRON-PRO-API-KEY", key);
        }
        let value: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
        // The node reports errors in a 200 body: {"Error": "..."} or
        // {"code": "...", "message": "<hex>"}.
        if let Some(err) = value.get("Error").and_then(|e| e.as_str()) {
            return Err(TronError::Api { code: "Error".into(), message: err.to_string() });
        }
        serde_json::from_value(value).map_err(|e| TronError::Decode(e.to_string()))
    }

    pub async fn get_now_block(&self) -> Result<NowBlock, TronError> {
        let v: serde_json::Value =
            self.post_json("/wallet/getnowblock", serde_json::json!({})).await?;
        parse_now_block(&v)
    }

    pub async fn tapos(&self) -> Result<(RefBlock, i64), TronError> {
        let nb = self.get_now_block().await?;
        Ok((ref_block(nb.number, &nb.block_id), nb.timestamp_ms))
    }
}

/// Parses a raw `/wallet/getnowblock` JSON response.
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

// `eprintln!` is used to document why live tests self-skip; it is disallowed
// in library code but appropriate for the test gate.
#[cfg(test)]
#[allow(clippy::disallowed_macros)]
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
