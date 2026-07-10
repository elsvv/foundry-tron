//! Transaction signing: secp256k1 over txid (= sha256(raw_data)),
//! 65-byte r||s||v signature with v = 27 + recovery_id (TronWeb format).

use crate::proto::{self, Transaction, TransactionRaw};
use alloy_primitives::{B256, hex};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use prost::Message;

/// A signed Tron transaction: the computed txid plus the protobuf
/// `Transaction` carrying the 65-byte signature.
pub struct SignedTronTx {
    pub txid: B256,
    pub tx: Transaction,
}

impl SignedTronTx {
    /// Hex-encodes the protobuf of the full `Transaction` for
    /// `POST /wallet/broadcasthex`.
    pub fn broadcast_hex(&self) -> String {
        hex::encode(self.tx.encode_to_vec())
    }
}

/// Signs `raw` with `signer`: computes txid = sha256(raw_data), signs it
/// with secp256k1, and stores a 65-byte `r||s||(27 + recovery_id)`
/// signature in the returned `Transaction`.
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
