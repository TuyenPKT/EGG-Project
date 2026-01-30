#![forbid(unsafe_code)]

use blake3::Hasher;
use egg_types::{canonical, Block, BlockHeader, Hash256, Transaction};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain(pub [u8; 16]);

impl Domain {
    pub const fn new(tag: [u8; 16]) -> Self {
        Self(tag)
    }
}

pub const DOMAIN_BLOCK_HEADER: Domain = Domain::new(*b"EGG:HDR:V0\0\0\0\0\0\0");
pub const DOMAIN_TX: Domain = Domain::new(*b"EGG:TX :V0\0\0\0\0\0\0");
pub const DOMAIN_BLOCK: Domain = Domain::new(*b"EGG:BLK:V0\0\0\0\0\0\0");

pub fn hash_domain(domain: Domain, bytes: &[u8]) -> Hash256 {
    let mut hasher = Hasher::new();
    hasher.update(&domain.0);
    hasher.update(bytes);
    let out = hasher.finalize();
    Hash256(*out.as_bytes())
}

pub fn hash_header(header: &BlockHeader) -> Hash256 {
    let enc = canonical::encode_block_header(header);
    hash_domain(DOMAIN_BLOCK_HEADER, &enc)
}

pub fn hash_tx(tx: &Transaction) -> Hash256 {
    let enc = canonical::encode_tx(tx);
    hash_domain(DOMAIN_TX, &enc)
}

pub fn hash_block(block: &Block) -> Hash256 {
    let enc = canonical::encode_block(block);
    hash_domain(DOMAIN_BLOCK, &enc)
}

pub fn leading_zero_bits(h: &Hash256) -> u32 {
    let mut count: u32 = 0;
    for b in h.0 {
        if b == 0 {
            count += 8;
            continue;
        }
        count += b.leading_zeros();
        break;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_types::{Height, Hash256};

    #[test]
    fn hash_is_deterministic_for_header() {
        let h = BlockHeader {
            parent: Hash256::zero(),
            height: Height(1),
            timestamp_utc: 1_700_000_000,
            nonce: 42,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 10,
        };
        assert_eq!(hash_header(&h), hash_header(&h));
    }

    #[test]
    fn domain_separates_hashes() {
        let h = BlockHeader {
            parent: Hash256::zero(),
            height: Height(1),
            timestamp_utc: 1_700_000_000,
            nonce: 42,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 10,
        };
        let tx = Transaction {
            id: Hash256::zero(),
            payload: b"hello".to_vec(),
        };

        let a = hash_header(&h);
        let b = hash_tx(&tx);
        assert_ne!(a, b);
    }

    #[test]
    fn leading_zero_bits_basic() {
        let h = Hash256([0u8; 32]);
        assert_eq!(leading_zero_bits(&h), 256);

        let mut x = [0u8; 32];
        x[0] = 0b0001_0000;
        let hx = Hash256(x);
        assert_eq!(leading_zero_bits(&hx), 3);
    }

    #[test]
    fn hash_block_is_deterministic() {
        let b = Block {
            header: BlockHeader {
                parent: Hash256::zero(),
                height: Height(1),
                timestamp_utc: 1_700_000_000,
                nonce: 1,
                merkle_root: Hash256::zero(),
                pow_difficulty_bits: 8,
            },
            txs: vec![Transaction {
                id: Hash256::zero(),
                payload: vec![1, 2, 3],
            }],
        };
        assert_eq!(hash_block(&b), hash_block(&b));
    }
}
