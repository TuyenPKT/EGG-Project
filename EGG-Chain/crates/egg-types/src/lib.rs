#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub const HASH256_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hash256(pub [u8; HASH256_LEN]);

impl Hash256 {
    pub fn zero() -> Self {
        Self([0u8; HASH256_LEN])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Height(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub parent: Hash256,
    pub height: Height,
    pub timestamp_utc: i64,
    pub nonce: u64,
    pub merkle_root: Hash256,
    pub pow_difficulty_bits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: Hash256,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub txs: Vec<Transaction>,
}

/// ChainSpec định nghĩa tham số mạng + genesis.
/// Mainnet_Official_Start = thời điểm genesis block (timestamp_utc, UTC).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSpec {
    pub spec_version: u32,
    pub chain: ChainParams,
    pub genesis: GenesisSpec,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainParams {
    pub chain_name: String,
    pub chain_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisSpec {
    /// UTC timestamp (seconds)
    pub timestamp_utc: i64,
    pub pow_difficulty_bits: u32,
    pub nonce: u64,
}

pub mod canonical {
    use super::{Block, BlockHeader, Hash256, Height, Transaction, HASH256_LEN};

    const MAGIC_HDR: [u8; 8] = *b"EGG_HDR0";
    const MAGIC_TX: [u8; 8] = *b"EGG_TX0\0";
    const MAGIC_BLK: [u8; 8] = *b"EGG_BLK0";

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CanonicalError {
        UnexpectedEof { at: usize, needed: usize, remaining: usize },
        InvalidMagic { at: usize },
        LengthOverflow { at: usize },
    }

    impl core::fmt::Display for CanonicalError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                CanonicalError::UnexpectedEof { at, needed, remaining } => write!(
                    f,
                    "unexpected eof at {} (needed {}, remaining {})",
                    at, needed, remaining
                ),
                CanonicalError::InvalidMagic { at } => write!(f, "invalid magic at {}", at),
                CanonicalError::LengthOverflow { at } => write!(f, "length overflow at {}", at),
            }
        }
    }

    impl std::error::Error for CanonicalError {}

    type Result<T> = core::result::Result<T, CanonicalError>;

    struct Cursor<'a> {
        buf: &'a [u8],
        pos: usize,
    }

    impl<'a> Cursor<'a> {
        fn new(buf: &'a [u8]) -> Self {
            Self { buf, pos: 0 }
        }

        fn remaining(&self) -> usize {
            self.buf.len().saturating_sub(self.pos)
        }

        fn take(&mut self, n: usize) -> Result<&'a [u8]> {
            let rem = self.remaining();
            if rem < n {
                return Err(CanonicalError::UnexpectedEof {
                    at: self.pos,
                    needed: n,
                    remaining: rem,
                });
            }
            let out = &self.buf[self.pos..self.pos + n];
            self.pos += n;
            Ok(out)
        }

        fn take_u32_be(&mut self) -> Result<u32> {
            let b = self.take(4)?;
            Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        }

        fn take_u64_be(&mut self) -> Result<u64> {
            let b = self.take(8)?;
            Ok(u64::from_be_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ]))
        }

        fn take_i64_be(&mut self) -> Result<i64> {
            let b = self.take(8)?;
            Ok(i64::from_be_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ]))
        }

        fn take_hash256(&mut self) -> Result<Hash256> {
            let b = self.take(HASH256_LEN)?;
            let mut out = [0u8; HASH256_LEN];
            out.copy_from_slice(b);
            Ok(Hash256(out))
        }

        fn expect_magic(&mut self, m: &[u8; 8]) -> Result<()> {
            let at = self.pos;
            let b = self.take(8)?;
            if b != m {
                return Err(CanonicalError::InvalidMagic { at });
            }
            Ok(())
        }
    }

    fn push_u32_be(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn push_u64_be(out: &mut Vec<u8>, v: u64) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn push_i64_be(out: &mut Vec<u8>, v: i64) {
        out.extend_from_slice(&v.to_be_bytes());
    }

    pub fn encode_block_header(h: &BlockHeader) -> Vec<u8> {
        // Fixed-size: 8 + 32 + 8 + 8 + 8 + 32 + 4 = 100 bytes
        let mut out = Vec::with_capacity(100);
        out.extend_from_slice(&MAGIC_HDR);
        out.extend_from_slice(&h.parent.0);
        push_u64_be(&mut out, h.height.0);
        push_i64_be(&mut out, h.timestamp_utc);
        push_u64_be(&mut out, h.nonce);
        out.extend_from_slice(&h.merkle_root.0);
        push_u32_be(&mut out, h.pow_difficulty_bits);
        out
    }

    pub fn decode_block_header(bytes: &[u8]) -> Result<BlockHeader> {
        let mut c = Cursor::new(bytes);
        c.expect_magic(&MAGIC_HDR)?;
        let parent = c.take_hash256()?;
        let height = Height(c.take_u64_be()?);
        let timestamp_utc = c.take_i64_be()?;
        let nonce = c.take_u64_be()?;
        let merkle_root = c.take_hash256()?;
        let pow_difficulty_bits = c.take_u32_be()?;
        Ok(BlockHeader {
            parent,
            height,
            timestamp_utc,
            nonce,
            merkle_root,
            pow_difficulty_bits,
        })
    }

    pub fn encode_tx(tx: &Transaction) -> Vec<u8> {
        // 8 + 32 + 4 + payload
        let payload_len_u32: u32 = tx
            .payload
            .len()
            .try_into()
            .unwrap_or(u32::MAX);
        let mut out = Vec::with_capacity(8 + 32 + 4 + tx.payload.len());
        out.extend_from_slice(&MAGIC_TX);
        out.extend_from_slice(&tx.id.0);
        push_u32_be(&mut out, payload_len_u32);
        out.extend_from_slice(&tx.payload);
        out
    }

    pub fn decode_tx(bytes: &[u8]) -> Result<Transaction> {
        let mut c = Cursor::new(bytes);
        c.expect_magic(&MAGIC_TX)?;
        let id = c.take_hash256()?;
        let payload_len = c.take_u32_be()? as usize;

        let rem = c.remaining();
        if rem < payload_len {
            return Err(CanonicalError::UnexpectedEof {
                at: c.pos,
                needed: payload_len,
                remaining: rem,
            });
        }
        let payload = c.take(payload_len)?.to_vec();
        Ok(Transaction { id, payload })
    }

    pub fn encode_block(b: &Block) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_BLK);
        out.extend_from_slice(&encode_block_header(&b.header));

        let tx_count_u32: u32 = b.txs.len().try_into().unwrap_or(u32::MAX);
        push_u32_be(&mut out, tx_count_u32);

        for tx in &b.txs {
            let tx_bytes = encode_tx(tx);
            let tx_len_u32: u32 = tx_bytes.len().try_into().unwrap_or(u32::MAX);
            push_u32_be(&mut out, tx_len_u32);
            out.extend_from_slice(&tx_bytes);
        }

        out
    }

    pub fn decode_block(bytes: &[u8]) -> Result<Block> {
        let mut c = Cursor::new(bytes);
        c.expect_magic(&MAGIC_BLK)?;

        let hdr_bytes = c.take(100)?;
        let header = decode_block_header(hdr_bytes)?;

        let tx_count = c.take_u32_be()? as usize;
        let mut txs = Vec::with_capacity(tx_count);

        for _ in 0..tx_count {
            let tx_len = c.take_u32_be()? as usize;
            if tx_len > c.remaining() {
                return Err(CanonicalError::UnexpectedEof {
                    at: c.pos,
                    needed: tx_len,
                    remaining: c.remaining(),
                });
            }
            let tx_bytes = c.take(tx_len)?;
            let tx = decode_tx(tx_bytes)?;
            txs.push(tx);
        }

        Ok(Block { header, txs })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{Hash256, Height};

        #[test]
        fn block_header_encoding_is_fixed_size() {
            let h = BlockHeader {
                parent: Hash256::zero(),
                height: Height(1),
                timestamp_utc: 1_700_000_000,
                nonce: 42,
                merkle_root: Hash256::zero(),
                pow_difficulty_bits: 10,
            };
            let b = encode_block_header(&h);
            assert_eq!(b.len(), 100);
        }

        #[test]
        fn block_header_roundtrip() {
            let h = BlockHeader {
                parent: Hash256::zero(),
                height: Height(2),
                timestamp_utc: 1_700_000_123,
                nonce: 999,
                merkle_root: Hash256::zero(),
                pow_difficulty_bits: 8,
            };
            let enc = encode_block_header(&h);
            let dec = decode_block_header(&enc).unwrap();
            assert_eq!(h, dec);
        }

        #[test]
        fn tx_roundtrip() {
            let tx = Transaction {
                id: Hash256::zero(),
                payload: vec![1, 2, 3, 4, 5],
            };
            let enc = encode_tx(&tx);
            let dec = decode_tx(&enc).unwrap();
            assert_eq!(tx, dec);
        }

        #[test]
        fn block_roundtrip() {
            let b = Block {
                header: BlockHeader {
                    parent: Hash256::zero(),
                    height: Height(3),
                    timestamp_utc: 1_700_000_999,
                    nonce: 7,
                    merkle_root: Hash256::zero(),
                    pow_difficulty_bits: 12,
                },
                txs: vec![
                    Transaction {
                        id: Hash256::zero(),
                        payload: vec![9, 9, 9],
                    },
                    Transaction {
                        id: Hash256::zero(),
                        payload: vec![1, 2, 3, 4],
                    },
                ],
            };

            let enc = encode_block(&b);
            let dec = decode_block(&enc).unwrap();
            assert_eq!(b, dec);
        }

        #[test]
        fn invalid_magic_rejected() {
            let bytes = vec![0u8; 100];
            let err = decode_block_header(&bytes).unwrap_err();
            assert!(matches!(err, CanonicalError::InvalidMagic { .. }));
        }
    }
}
