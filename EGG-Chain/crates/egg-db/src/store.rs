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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockMeta {
    pub parent: Hash256,
    pub height: Height,
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

    fn put_block_meta(&self, id: Hash256, meta: BlockMeta) -> Result<()>;
    fn get_block_meta(&self, id: Hash256) -> Result<Option<BlockMeta>>;

    fn add_child(&self, parent: Hash256, child: Hash256) -> Result<()>;
    fn get_children(&self, parent: Hash256) -> Result<Vec<Hash256>>;

    fn set_canon_hash(&self, height: Height, hash: Hash256) -> Result<()>;
    fn get_canon_hash(&self, height: Height) -> Result<Option<Hash256>>;
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

    fn k_block_meta(id: Hash256) -> Vec<u8> {
        let mut k = Vec::with_capacity(6 + 32);
        k.extend_from_slice(b"bmeta:");
        k.extend_from_slice(&id.0);
        k
    }

    fn k_children(parent: Hash256) -> Vec<u8> {
        let mut k = Vec::with_capacity(6 + 32);
        k.extend_from_slice(b"child:");
        k.extend_from_slice(&parent.0);
        k
    }

    fn k_canon(height: Height) -> Vec<u8> {
        let mut k = Vec::with_capacity(6 + 8);
        k.extend_from_slice(b"canon:");
        k.extend_from_slice(&height.0.to_be_bytes());
        k
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

    fn encode_block_meta(meta: BlockMeta) -> Vec<u8> {
        const MAGIC: [u8; 8] = *b"EGG_BM00";
        let mut out = Vec::with_capacity(8 + 32 + 8);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&meta.parent.0);
        out.extend_from_slice(&meta.height.0.to_be_bytes());
        out
    }

    fn decode_block_meta(bytes: &[u8]) -> Result<BlockMeta> {
        const MAGIC: [u8; 8] = *b"EGG_BM00";
        if bytes.len() < 8 + 32 + 8 {
            return Err(StoreError::Decode("bmeta: unexpected eof".to_string()));
        }
        if &bytes[0..8] != MAGIC {
            return Err(StoreError::Decode("bmeta: invalid magic".to_string()));
        }
        let mut parent = [0u8; 32];
        parent.copy_from_slice(&bytes[8..40]);

        let h_bytes: [u8; 8] = bytes[40..48]
            .try_into()
            .map_err(|_| StoreError::Decode("bmeta: bad height bytes".to_string()))?;
        let height = Height(u64::from_be_bytes(h_bytes));

        Ok(BlockMeta {
            parent: Hash256(parent),
            height,
        })
    }

    fn encode_children(children: &[Hash256]) -> Vec<u8> {
        const MAGIC: [u8; 8] = *b"EGG_CH00";
        let mut out = Vec::with_capacity(8 + 4 + 32 * children.len());
        out.extend_from_slice(&MAGIC);
        let n: u32 = children.len().try_into().unwrap_or(u32::MAX);
        out.extend_from_slice(&n.to_be_bytes());
        for h in children {
            out.extend_from_slice(&h.0);
        }
        out
    }

    fn decode_children(bytes: &[u8]) -> Result<Vec<Hash256>> {
        const MAGIC: [u8; 8] = *b"EGG_CH00";
        if bytes.len() < 8 + 4 {
            return Err(StoreError::Decode("child: unexpected eof".to_string()));
        }
        if &bytes[0..8] != MAGIC {
            return Err(StoreError::Decode("child: invalid magic".to_string()));
        }
        let n_bytes: [u8; 4] = bytes[8..12]
            .try_into()
            .map_err(|_| StoreError::Decode("child: bad count bytes".to_string()))?;
        let n = u32::from_be_bytes(n_bytes) as usize;

        let expect = 8 + 4 + 32 * n;
        if bytes.len() != expect {
            return Err(StoreError::Decode("child: length mismatch".to_string()));
        }

        let mut out = Vec::with_capacity(n);
        let mut off = 12usize;
        for _ in 0..n {
            let mut h = [0u8; 32];
            h.copy_from_slice(&bytes[off..off + 32]);
            out.push(Hash256(h));
            off += 32;
        }
        Ok(out)
    }

    fn encode_canon(hash: Hash256) -> Vec<u8> {
        const MAGIC: [u8; 8] = *b"EGG_CA00";
        let mut out = Vec::with_capacity(8 + 32);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&hash.0);
        out
    }

    fn decode_canon(bytes: &[u8]) -> Result<Hash256> {
        const MAGIC: [u8; 8] = *b"EGG_CA00";
        if bytes.len() < 8 + 32 {
            return Err(StoreError::Decode("canon: unexpected eof".to_string()));
        }
        if &bytes[0..8] != MAGIC {
            return Err(StoreError::Decode("canon: invalid magic".to_string()));
        }
        let mut h = [0u8; 32];
        h.copy_from_slice(&bytes[8..40]);
        Ok(Hash256(h))
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

    fn put_block_meta(&self, id: Hash256, meta: BlockMeta) -> Result<()> {
        let key = Self::k_block_meta(id);
        let val = Self::encode_block_meta(meta);
        self.kv.put(key, val)?;
        Ok(())
    }

    fn get_block_meta(&self, id: Hash256) -> Result<Option<BlockMeta>> {
        let key = Self::k_block_meta(id);
        if !self.kv.has(&key)? {
            return Ok(None);
        }
        let val = self.kv.get(&key)?;
        Ok(Some(Self::decode_block_meta(&val)?))
    }

    fn add_child(&self, parent: Hash256, child: Hash256) -> Result<()> {
        let key = Self::k_children(parent);
        let mut children = if self.kv.has(&key)? {
            let val = self.kv.get(&key)?;
            Self::decode_children(&val)?
        } else {
            Vec::new()
        };

        if !children.iter().any(|h| *h == child) {
            children.push(child);
            let val = Self::encode_children(&children);
            self.kv.put(key, val)?;
        }

        Ok(())
    }

    fn get_children(&self, parent: Hash256) -> Result<Vec<Hash256>> {
        let key = Self::k_children(parent);
        if !self.kv.has(&key)? {
            return Ok(Vec::new());
        }
        let val = self.kv.get(&key)?;
        Self::decode_children(&val)
    }

    fn set_canon_hash(&self, height: Height, hash: Hash256) -> Result<()> {
        let key = Self::k_canon(height);
        let val = Self::encode_canon(hash);
        self.kv.put(key, val)?;
        Ok(())
    }

    fn get_canon_hash(&self, height: Height) -> Result<Option<Hash256>> {
        let key = Self::k_canon(height);
        if !self.kv.has(&key)? {
            return Ok(None);
        }
        let val = self.kv.get(&key)?;
        Ok(Some(Self::decode_canon(&val)?))
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

    #[test]
    fn store_block_meta_roundtrip() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);

        let id = Hash256([5u8; 32]);
        let m = BlockMeta {
            parent: Hash256([6u8; 32]),
            height: Height(7),
        };

        assert_eq!(store.get_block_meta(id).unwrap(), None);
        store.put_block_meta(id, m).unwrap();

        let back = store.get_block_meta(id).unwrap().expect("bmeta exists");
        assert_eq!(m, back);
    }

    #[test]
    fn children_index_roundtrip_and_is_idempotent() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);

        let p = Hash256([1u8; 32]);
        let c1 = Hash256([2u8; 32]);
        let c2 = Hash256([3u8; 32]);

        assert!(store.get_children(p).unwrap().is_empty());

        store.add_child(p, c1).unwrap();
        store.add_child(p, c2).unwrap();
        store.add_child(p, c1).unwrap(); // idempotent

        let children = store.get_children(p).unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0], c1);
        assert_eq!(children[1], c2);
    }

    #[test]
    fn canon_height_roundtrip() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);

        let h = Height(10);
        let x = Hash256([8u8; 32]);

        assert_eq!(store.get_canon_hash(h).unwrap(), None);
        store.set_canon_hash(h, x).unwrap();
        assert_eq!(store.get_canon_hash(h).unwrap(), Some(x));
    }
}
