//! TAPOS (Transaction as Proof of Stake) reference-block fields.

pub struct RefBlock {
    pub bytes: Vec<u8>,
    pub hash: Vec<u8>,
}

pub fn ref_block(block_number: i64, block_id: &[u8; 32]) -> RefBlock {
    let be = block_number.to_be_bytes();
    RefBlock { bytes: be[6..8].to_vec(), hash: block_id[8..16].to_vec() }
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
        // блок 63156856 = 0x03C3B278 -> [..,0xB2,0x78]
        let rb = ref_block(63_156_856, &[0u8; 32]);
        assert_eq!(rb.bytes, vec![0xb2, 0x78]);
    }
}
