#![forbid(unsafe_code)]

use egg_crypto::{hash_header, leading_zero_bits};
use egg_types::{BlockHeader, Hash256};

pub mod chainspec;
pub mod state;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowPolicy {
    pub difficulty_bits: u32,
}

impl PowPolicy {
    pub fn new(difficulty_bits: u32) -> Self {
        Self { difficulty_bits }
    }
}

pub fn header_id(header: &BlockHeader) -> Hash256 {
    hash_header(header)
}

pub fn pow_valid(header: &BlockHeader) -> bool {
    let id = header_id(header);
    leading_zero_bits(&id) >= header.pow_difficulty_bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_types::{Height, Hash256};

    #[test]
    fn header_id_is_deterministic() {
        let h = BlockHeader {
            parent: Hash256::zero(),
            height: Height(1),
            timestamp_utc: 1_700_000_000,
            nonce: 0,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 8,
        };
        assert_eq!(header_id(&h), header_id(&h));
    }

    #[test]
    fn mine_low_difficulty_pow() {
        let mut h = BlockHeader {
            parent: Hash256::zero(),
            height: Height(1),
            timestamp_utc: 1_700_000_000,
            nonce: 0,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 8,
        };

        let mut tries: u64 = 0;
        loop {
            if pow_valid(&h) {
                break;
            }
            h.nonce = h.nonce.wrapping_add(1);
            tries += 1;
            if tries > 5_000_000 {
                panic!("mine test exceeded tries");
            }
        }

        assert!(pow_valid(&h));
    }

    #[test]
    fn pow_policy_struct_exists() {
        let p = PowPolicy::new(16);
        assert_eq!(p.difficulty_bits, 16);
    }
}
