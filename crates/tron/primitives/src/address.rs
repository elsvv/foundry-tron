//! Tron address codec.
//!
//! Internally addresses are 20-byte [`Address`]; the 0x41 prefix and
//! base58check exist only at I/O boundaries.

use alloy_primitives::{Address, hex};
use sha2::{Digest, Sha256};

pub const TRON_ADDRESS_PREFIX: u8 = 0x41;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
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
    let decoded =
        bs58::decode(s).into_vec().map_err(|e| AddressError::InvalidBase58(e.to_string()))?;
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
        assert!(matches!(
            parse("4200000000000000000000000000000000000000ff"),
            Err(AddressError::InvalidPrefix(0x42))
        ));
        assert!(matches!(
            parse("0x1234"),
            Err(AddressError::InvalidHex(_) | AddressError::InvalidLength(_))
        ));
    }
}
