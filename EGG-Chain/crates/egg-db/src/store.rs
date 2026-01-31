#![forbid(unsafe_code)]

use egg_types::{canonical, Block, BlockHeader, Hash256, Height};
use thiserror::Error;

use crate::{DbError, KvStore};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("db error: {0}")]
    Db(#[from] DbError),

    #[error("decode error: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainTip {
    pub height: Height,
    pub hash: Hash256,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainMeta {
    pub chain_id: u32,
    pub genesis_id: Hash256,
    pub chainspec_hash: Hash256,
}

pub trait BlockStore {
    fn put_header(&self, id: Hash256, header: &BlockHeader) -> Result<()>;
    fn get_header(&self, id: Hash256) -> Result<BlockHeader>;
    fn has_header(&self, id: Hash256) -> Result<bool>;

    fn put_block(&self, id: Hash256, block: &Block) -> Result<()>;
    fn get_block(&self, id: Hash256) -> Result<Block>;
    fn has_block(&self, id: Hash256) -> Result<bool>;
}

pub trait ChainStore: BlockStore {
    fn set_tip(&self, tip: ChainTip) -> Result<()>;
    fn get_tip(&self) -> Result<Option<ChainTip>>;

    fn set_meta(&self, meta: ChainMeta) -> Result<()>;
    fn get_meta(&self) -> Result<Option<ChainMeta>>;
}

#[derive(Clone)]
pub struct DbChainStore<S: KvStore> {
    kv: S,
}

impl<S: KvStore> DbChainStore<S> {
    pub fn new(kv: S) -> Self {
        Self { kv }
    }

    fn k_header(id: Hash256) -> Vec<u8> {
        let mut k = Vec::with_capacity(4 + 32);
        k.extend_from_slice(b"hdr:");
        k.extend_from_slice(&id.0);
        k
    }

    fn k_block(id: Hash256) -> Vec<u8> {
        let mut k = Vec::with_capacity(4 + 32);
        k.extend_from_slice(b"blk:");
        k.extend_from_slice(&id.0);
        k
    }

    fn k_tip() -> &'static [u8] {
        b"tip:"
    }

    fn k_meta() -> &'static [u8] {
        b"meta:"
    }

    fn encode_tip(tip: ChainTip) -> Vec<u8> {
        const MAGIC: [u8; 8] = *b"EGG_TIP0";
        let mut out = Vec::with_capacity(48);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&tip.height.0.to_be_bytes());
        out.extend_from_slice(&tip.hash.0);
        out
    }

    fn decode_tip(bytes: &[u8]) -> Result<ChainTip> {
        const MAGIC: [u8; 8] = *b"EGG_TIP0";
        if bytes.len() < 8 + 8 + 32 {
            return Err(StoreError::Decode("tip: unexpected eof".to_string()));
        }
        if &bytes[0..8] != MAGIC {
            return Err(StoreError::Decode("tip: invalid magic".to_string()));
        }
        let h_bytes: [u8; 8] = bytes[8..16]
            .try_into()
            .map_err(|_| StoreError::Decode("tip: bad height bytes".to_string()))?;
        let height = Height(u64::from_be_bytes(h_bytes));

        let hash_slice = &bytes[16..48];
        let mut hash = [0u8; 32];
        hash.copy_from_slice(hash_slice);
        Ok(ChainTip {
            height,
            hash: Hash256(hash),
        })
    }

    fn encode_meta(meta: ChainMeta) -> Vec<u8> {
        const MAGIC: [u8; 8] = *b"EGG_MET0";
        let mut out = Vec::with_capacity(8 + 4 + 32 + 32);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&meta.chain_id.to_be_bytes());
        out.extend_from_slice(&meta.genesis_id.0);
        out.extend_from_slice(&meta.chainspec_hash.0);
        out
    }

    fn decode_meta(bytes: &[u8]) -> Result<ChainMeta> {
        const MAGIC: [u8; 8] = *b"EGG_MET0";
        if bytes.len() < 8 + 4 + 32 + 32 {
            return Err(StoreError::Decode("meta: unexpected eof".to_string()));
        }
        if &bytes[0..8] != MAGIC {
            return Err(StoreError::Decode("meta: invalid magic".to_string()));
        }
        let cid_bytes: [u8; 4] = bytes[8..12]
            .try_into()
            .map_err(|_| StoreError::Decode("meta: bad chain_id bytes".to_string()))?;
        let chain_id = u32::from_be_bytes(cid_bytes);

        let mut genesis_id = [0u8; 32];
        genesis_id.copy_from_slice(&bytes[12..44]);

        let mut chainspec_hash = [0u8; 32];
        chainspec_hash.copy_from_slice(&bytes[44..76]);

        Ok(ChainMeta {
            chain_id,
            genesis_id: Hash256(genesis_id),
            chainspec_hash: Hash256(chainspec_hash),
        })
    }
}

impl<S: KvStore> BlockStore for DbChainStore<S> {
    fn put_header(&self, id: Hash256, header: &BlockHeader) -> Result<()> {
        let key = Self::k_header(id);
        let val = canonical::encode_block_header(header);
        self.kv.put(key, val)?;
        Ok(())
    }

    fn get_header(&self, id: Hash256) -> Result<BlockHeader> {
        let key = Self::k_header(id);
        let val = self.kv.get(&key)?;
        canonical::decode_block_header(&val)
            .map_err(|e| StoreError::Decode(format!("header decode: {}", e)))
    }

    fn has_header(&self, id: Hash256) -> Result<bool> {
        Ok(self.kv.has(&Self::k_header(id))?)
    }

    fn put_block(&self, id: Hash256, block: &Block) -> Result<()> {
        let key = Self::k_block(id);
        let val = canonical::encode_block(block);
        self.kv.put(key, val)?;
        Ok(())
    }

    fn get_block(&self, id: Hash256) -> Result<Block> {
        let key = Self::k_block(id);
        let val = self.kv.get(&key)?;
        canonical::decode_block(&val).map_err(|e| StoreError::Decode(format!("block decode: {}", e)))
    }

    fn has_block(&self, id: Hash256) -> Result<bool> {
        Ok(self.kv.has(&Self::k_block(id))?)
    }
}

impl<S: KvStore> ChainStore for DbChainStore<S> {
    fn set_tip(&self, tip: ChainTip) -> Result<()> {
        let key = Self::k_tip().to_vec();
        let val = Self::encode_tip(tip);
        self.kv.put(key, val)?;
        Ok(())
    }

    fn get_tip(&self) -> Result<Option<ChainTip>> {
        let key = Self::k_tip();
        if !self.kv.has(key)? {
            return Ok(None);
        }
        let val = self.kv.get(key)?;
        Ok(Some(Self::decode_tip(&val)?))
    }

    fn set_meta(&self, meta: ChainMeta) -> Result<()> {
        let key = Self::k_meta().to_vec();
        let val = Self::encode_meta(meta);
        self.kv.put(key, val)?;
        Ok(())
    }

    fn get_meta(&self) -> Result<Option<ChainMeta>> {
        let key = Self::k_meta();
        if !self.kv.has(key)? {
            return Ok(None);
        }
        let val = self.kv.get(key)?;
        Ok(Some(Self::decode_meta(&val)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemKv;

    fn sample_header() -> BlockHeader {
        BlockHeader {
            parent: Hash256::zero(),
            height: Height(0),
            timestamp_utc: 1_700_000_000,
            nonce: 0,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 0,
        }
    }

    #[test]
    fn store_header_roundtrip() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);

        let hdr = sample_header();
        let id = Hash256([1u8; 32]);

        store.put_header(id, &hdr).unwrap();
        assert_eq!(store.has_header(id).unwrap(), true);

        let back = store.get_header(id).unwrap();
        assert_eq!(hdr, back);
    }

    #[test]
    fn store_block_roundtrip() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);

        let blk = Block {
            header: sample_header(),
            txs: vec![],
        };
        let id = Hash256([2u8; 32]);

        store.put_block(id, &blk).unwrap();
        assert_eq!(store.has_block(id).unwrap(), true);

        let back = store.get_block(id).unwrap();
        assert_eq!(blk, back);
    }

    #[test]
    fn store_tip_roundtrip() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);

        let tip = ChainTip {
            height: Height(123),
            hash: Hash256([9u8; 32]),
        };

        assert_eq!(store.get_tip().unwrap(), None);
        store.set_tip(tip).unwrap();

        let back = store.get_tip().unwrap().expect("tip exists");
        assert_eq!(tip, back);
    }

    #[test]
    fn store_meta_roundtrip() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);

        let meta = ChainMeta {
            chain_id: 1,
            genesis_id: Hash256([3u8; 32]),
            chainspec_hash: Hash256([4u8; 32]),
        };

        assert_eq!(store.get_meta().unwrap(), None);
        store.set_meta(meta).unwrap();

        let back = store.get_meta().unwrap().expect("meta exists");
        assert_eq!(meta, back);
    }
}
