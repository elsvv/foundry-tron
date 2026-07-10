//! Key-to-address helpers. Tron derives the 20-byte body exactly like
//! Ethereum (keccak256(uncompressed_pubkey)[12..]); only the display
//! encoding (0x41 + base58check) differs.

use crate::address;
use alloy_signer_local::PrivateKeySigner;

pub fn signer_base58_address(signer: &PrivateKeySigner) -> String {
    address::to_base58(signer.address())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // Известный Ethereum-вектор: тело адреса у Tron то же самое.
    // Адрес получен независимо (secp256k1 + keccak256), совпадает с alloy;
    // EIP-55 checksum как у alloy `to_checksum(None)`.
    const PRIVKEY: &str = "4c0883a69102937d6231471b5dbb6204fe512961708279feb1be6ae5538da033";
    const ETH_ADDR: &str = "0xb960bED53c17f9a021538b5d6f08e7466B966c53";

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
