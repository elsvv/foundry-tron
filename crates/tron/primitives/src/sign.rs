//! Transaction signing: secp256k1 over txid (= sha256(raw_data)),
//! 65-byte r||s||v signature with v = 27 + recovery_id (TronWeb format).

use crate::proto::{self, Transaction, TransactionRaw};
use alloy_primitives::{B256, Signature, hex};
use alloy_signer::{Signer, SignerSync};
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

/// Packs a `raw`/`txid` pair and its secp256k1 `Signature` into a
/// `SignedTronTx` with the 65-byte `r||s||(27 + recovery_id)` layout java-tron
/// expects (TronWeb format).
fn pack_signed(raw: TransactionRaw, txid: B256, sig: Signature) -> SignedTronTx {
    let mut bytes = Vec::with_capacity(65);
    bytes.extend_from_slice(&sig.r().to_be_bytes::<32>());
    bytes.extend_from_slice(&sig.s().to_be_bytes::<32>());
    bytes.push(27 + sig.v() as u8);
    SignedTronTx { txid, tx: Transaction { raw_data: Some(raw), signature: vec![bytes] } }
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
    Ok(pack_signed(raw, txid, sig))
}

/// Async equivalent of [`sign_raw`], generic over any [`alloy_signer::Signer`].
///
/// `foundry_wallets::WalletSigner` implements only the async `Signer` trait (no
/// `SignerSync`), so on-chain deploy/broadcast paths — which must work for every
/// wallet backend — sign through this. The 65-byte packing is identical, and for
/// a `PrivateKeySigner` the RFC 6979 deterministic nonce makes the resulting
/// signature byte-for-byte equal to [`sign_raw`].
pub async fn sign_raw_with<S: Signer + ?Sized>(
    raw: TransactionRaw,
    signer: &S,
) -> alloy_signer::Result<SignedTronTx> {
    let txid = proto::txid(&raw);
    let sig = signer.sign_hash(&txid).await?;
    Ok(pack_signed(raw, txid, sig))
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

    #[tokio::test]
    async fn sign_raw_with_matches_sign_raw() {
        let signer = PrivateKeySigner::from_str(
            "4c0883a69102937d6231471b5dbb6204fe512961708279feb1be6ae5538da033",
        )
        .unwrap();
        let sync_signed = sign_raw(sample_raw(), &signer).unwrap();
        let async_signed = sign_raw_with(sample_raw(), &signer).await.unwrap();

        // Deterministic secp256k1 (RFC 6979): identical txid and 65 bytes.
        assert_eq!(async_signed.txid, sync_signed.txid);
        assert_eq!(async_signed.tx.signature, sync_signed.tx.signature);

        let sig = &async_signed.tx.signature[0];
        assert_eq!(sig.len(), 65);
        let rs = alloy_primitives::Signature::from_raw(sig).unwrap();
        let recovered = rs.recover_address_from_prehash(&async_signed.txid).unwrap();
        assert_eq!(recovered, signer.address());
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
