#![forbid(unsafe_code)]

use std::collections::VecDeque;

use egg_crypto::hash_chainspec;
use egg_db::store::{BlockMeta, ChainMeta, ChainStore, ChainTip, StoreError};
use egg_types::{Block, BlockHeader, ChainSpec, Hash256, Height};
use thiserror::Error;

use crate::block_builder::BlockBuildError;
use crate::chainspec::{genesis_id, genesis_header, validate_chainspec, ChainSpecError};
use crate::{header_id, pow_valid};

#[derive(Debug, Error)]
pub enum ChainStateError {
    #[error("chainspec error: {0}")]
    Spec(#[from] ChainSpecError),

    #[error("store error: {0}")]
    Store(#[from] StoreError),

    #[error("mining error: {0}")]
    Mining(#[from] crate::miner::MiningError),

    #[error("block build error: {0}")]
    BlockBuild(#[from] BlockBuildError),

    #[error("chainstore missing meta (required)")]
    MetaMissing,

    #[error("chain meta mismatch: expected={expected:?} got={got:?}")]
    MetaMismatch { expected: ChainMeta, got: ChainMeta },

    #[error("genesis header mismatch between spec and stored data")]
    GenesisHeaderMismatch,

    #[error("invalid pow for header")]
    InvalidPow,

    #[error("block height does not match parent+1: parent_height={parent_height:?} child_height={child_height:?}")]
    HeightNotParentPlusOne { parent_height: Height, child_height: Height },

    #[error("genesis id mismatch: expected {expected:?}, got {got:?}")]
    GenesisIdMismatch { expected: Hash256, got: Hash256 },

    #[error("missing header for block {id:?}")]
    MissingHeader { id: Hash256 },

    #[error("missing block for block {id:?}")]
    MissingBlock { id: Hash256 },

    #[error("missing block meta for block {id:?}")]
    MissingBlockMeta { id: Hash256 },

    #[error("block header does not match stored header for id {id:?}")]
    HeaderMismatch { id: Hash256 },
}

pub type Result<T> = std::result::Result<T, ChainStateError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestOutcome {
    AlreadyKnown,
    StoredOrphan,
    StoredConnected,
    NewTip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderIngestOutcome {
    AlreadyKnown,
    StoredOrphan,
    StoredConnected,
}

#[derive(Clone)]
pub struct ChainState<S: ChainStore + Clone> {
    pub spec: ChainSpec,
    pub tip: ChainTip,
    pub meta: ChainMeta,
    store: S,
}

impl<S: ChainStore + Clone> ChainState<S> {
    pub fn store(&self) -> &S {
        &self.store
    }

    fn expected_meta(spec: &ChainSpec) -> Result<ChainMeta> {
        let gid = genesis_id(spec)?;
        Ok(ChainMeta {
            chain_id: spec.chain.chain_id,
            genesis_id: gid,
            chainspec_hash: hash_chainspec(spec),
        })
    }

    fn hash_lt(a: Hash256, b: Hash256) -> bool {
        a.0 < b.0
    }

    fn ensure_block_meta_from_header(&self, id: Hash256, hdr: &BlockHeader) -> Result<BlockMeta> {
        if let Some(m) = self.store.get_block_meta(id)? {
            return Ok(m);
        }
        let m = BlockMeta {
            parent: hdr.parent,
            height: hdr.height,
        };
        self.store.put_block_meta(id, m)?;
        Ok(m)
    }

    fn must_block_meta(&self, id: Hash256) -> Result<BlockMeta> {
        self.store
            .get_block_meta(id)?
            .ok_or(ChainStateError::MissingBlockMeta { id })
    }

    fn must_header(&self, id: Hash256) -> Result<BlockHeader> {
        if !self.store.has_header(id)? {
            return Err(ChainStateError::MissingHeader { id });
        }
        Ok(self.store.get_header(id)?)
    }

    fn must_block(&self, id: Hash256) -> Result<Block> {
        if !self.store.has_block(id)? {
            return Err(ChainStateError::MissingBlock { id });
        }
        Ok(self.store.get_block(id)?)
    }

    fn bootstrap_indexes_from_tip(&self, tip: ChainTip) -> Result<()> {
        let need_bmeta = self.store.get_block_meta(tip.hash)?.is_none();
        let need_canon = self.store.get_canon_hash(tip.height)?.is_none();

        if !(need_bmeta || need_canon) {
            return Ok(());
        }

        let mut cur = tip.hash;
        loop {
            let hdr = self.must_header(cur)?;
            self.ensure_block_meta_from_header(cur, &hdr)?;
            self.store.set_canon_hash(hdr.height, cur)?;

            if hdr.height == Height(0) {
                break;
            }

            self.store.add_child(hdr.parent, cur)?;
            cur = hdr.parent;
        }

        Ok(())
    }

    pub fn open_or_init(store: S, spec: ChainSpec) -> Result<Self> {
        validate_chainspec(&spec)?;
        let expected = Self::expected_meta(&spec)?;

        match store.get_tip()? {
            Some(tip) => {
                let got = store.get_meta()?.ok_or(ChainStateError::MetaMissing)?;
                if got != expected {
                    return Err(ChainStateError::MetaMismatch { expected, got });
                }

                let st = Self {
                    spec,
                    tip,
                    meta: got,
                    store,
                };
                st.bootstrap_indexes_from_tip(tip)?;
                Ok(st)
            }
            None => {
                let hdr = genesis_header(&spec)?;
                let gid = expected.genesis_id;

                let computed = header_id(&hdr);
                if computed != gid {
                    return Err(ChainStateError::GenesisIdMismatch {
                        expected: gid,
                        got: computed,
                    });
                }

                let blk = egg_types::Block {
                    header: hdr.clone(),
                    txs: vec![],
                };

                store.set_meta(expected)?;
                store.put_header(gid, &hdr)?;
                store.put_block(gid, &blk)?;

                store.put_block_meta(
                    gid,
                    BlockMeta {
                        parent: hdr.parent,
                        height: hdr.height,
                    },
                )?;
                store.set_canon_hash(Height(0), gid)?;

                let tip = ChainTip {
                    height: Height(0),
                    hash: gid,
                };
                store.set_tip(tip)?;

                Ok(Self {
                    spec,
                    tip,
                    meta: expected,
                    store,
                })
            }
        }
    }

    pub fn verify_genesis_matches_spec(&self) -> Result<()> {
        let gid = self.meta.genesis_id;
        let hdr_expected = genesis_header(&self.spec)?;
        let hdr_stored = self.store.get_header(gid)?;
        if hdr_stored != hdr_expected {
            return Err(ChainStateError::GenesisHeaderMismatch);
        }
        Ok(())
    }

    pub fn canon_hash(&self, height: Height) -> Result<Option<Hash256>> {
        Ok(self.store.get_canon_hash(height)?)
    }

    pub fn get_headers_after(&self, start_hash: Hash256, max: usize) -> Result<Vec<BlockHeader>> {
        if max == 0 {
            return Ok(vec![]);
        }

        let mut start_h: Option<u64> = None;
        for h in 0..=self.tip.height.0 {
            let hh = self.store.get_canon_hash(Height(h))?;
            if let Some(x) = hh {
                if x == start_hash {
                    start_h = Some(h);
                    break;
                }
            }
        }
        let Some(sh) = start_h else {
            return Ok(vec![]);
        };

        let mut out = Vec::new();
        let mut cur_h = sh.saturating_add(1);
        while cur_h <= self.tip.height.0 && out.len() < max {
            let Some(hh) = self.store.get_canon_hash(Height(cur_h))? else { break; };
            let hdr = self.store.get_header(hh)?;
            out.push(hdr);
            cur_h = cur_h.saturating_add(1);
        }

        Ok(out)
    }

    fn reorg_canonical(&self, old_tip: ChainTip, new_tip: ChainTip) -> Result<()> {
        let mut a = new_tip.hash;
        let mut ha = new_tip.height.0;
        let mut b = old_tip.hash;
        let mut hb = old_tip.height.0;

        while ha > hb {
            let m = self.must_block_meta(a)?;
            a = m.parent;
            ha = ha.saturating_sub(1);
        }
        while hb > ha {
            let m = self.must_block_meta(b)?;
            b = m.parent;
            hb = hb.saturating_sub(1);
        }
        while a != b {
            let ma = self.must_block_meta(a)?;
            let mb = self.must_block_meta(b)?;
            a = ma.parent;
            b = mb.parent;
            ha = ha.saturating_sub(1);
            hb = hb.saturating_sub(1);
        }
        let ancestor_height = Height(ha);

        let mut path: Vec<(Height, Hash256)> = Vec::new();
        let mut cur = new_tip.hash;
        loop {
            let m = self.must_block_meta(cur)?;
            if m.height == ancestor_height {
                break;
            }
            path.push((m.height, cur));
            cur = m.parent;
        }
        path.reverse();

        for (h, x) in path {
            self.store.set_canon_hash(h, x)?;
        }

        Ok(())
    }

    fn maybe_set_tip(&mut self, candidate_hash: Hash256, candidate_height: Height) -> Result<bool> {
        let better = if candidate_height.0 > self.tip.height.0 {
            true
        } else if candidate_height.0 == self.tip.height.0 {
            Self::hash_lt(candidate_hash, self.tip.hash)
        } else {
            false
        };

        if !better {
            return Ok(false);
        }

        let old = self.tip;
        let new_tip = ChainTip {
            height: candidate_height,
            hash: candidate_hash,
        };

        self.store.set_tip(new_tip)?;
        self.tip = new_tip;

        self.reorg_canonical(old, new_tip)?;
        Ok(true)
    }

    fn try_connect_child(&mut self, parent: Hash256, child: Hash256) -> Result<bool> {
        if !self.store.has_header(child)? || !self.store.has_block(child)? {
            return Ok(false);
        }

        let child_hdr = self.store.get_header(child)?;
        if child_hdr.parent != parent {
            return Ok(false);
        }

        let parent_hdr = self.store.get_header(parent)?;
        let parent_meta = self.ensure_block_meta_from_header(parent, &parent_hdr)?;
        let child_meta = self.ensure_block_meta_from_header(child, &child_hdr)?;

        let expect_h = Height(parent_meta.height.0.saturating_add(1));
        if child_meta.height != expect_h || child_hdr.height != expect_h {
            return Err(ChainStateError::HeightNotParentPlusOne {
                parent_height: parent_meta.height,
                child_height: child_hdr.height,
            });
        }

        let _ = self.maybe_set_tip(child, child_meta.height)?;
        Ok(true)
    }

    fn connect_descendants_from(&mut self, root: Hash256) -> Result<()> {
        let mut q = VecDeque::new();
        q.push_back(root);

        while let Some(p) = q.pop_front() {
            let children = self.store.get_children(p)?;
            for c in children {
                let connected = self.try_connect_child(p, c)?;
                if connected {
                    q.push_back(c);
                }
            }
        }
        Ok(())
    }

    pub fn ingest_block(&mut self, block: Block) -> Result<(Hash256, IngestOutcome)> {
        crate::block_builder::verify_block_merkle(&block)?;

        if !pow_valid(&block.header) {
            return Err(ChainStateError::InvalidPow);
        }

        let id = header_id(&block.header);

        if block.header.height == Height(0) {
            if id != self.meta.genesis_id {
                return Err(ChainStateError::GenesisIdMismatch {
                    expected: self.meta.genesis_id,
                    got: id,
                });
            }
            // genesis đã có khi open_or_init; nếu ai đó gửi lại genesis block thì coi như known
            return Ok((id, IngestOutcome::AlreadyKnown));
        }

        // CASE: header đã có từ headers-first, nhưng block chưa có -> phải cho phép put_block + connect.
        if self.store.has_header(id)? {
            if self.store.has_block(id)? {
                return Ok((id, IngestOutcome::AlreadyKnown));
            }

            let stored_hdr = self.store.get_header(id)?;
            if stored_hdr != block.header {
                return Err(ChainStateError::HeaderMismatch { id });
            }

            self.store.put_block(id, &block)?;
            self.ensure_block_meta_from_header(id, &block.header)?;

            // đảm bảo parent->children index
            let p = block.header.parent;
            let existing_children = self.store.get_children(p)?;
            if !existing_children.iter().any(|x| *x == id) {
                self.store.add_child(p, id)?;
            }

            if !self.store.has_header(p)? {
                return Ok((id, IngestOutcome::StoredOrphan));
            }

            let parent_hdr = self.store.get_header(p)?;
            let parent_meta = self.ensure_block_meta_from_header(p, &parent_hdr)?;
            let expect_h = Height(parent_meta.height.0.saturating_add(1));
            if block.header.height != expect_h {
                return Err(ChainStateError::HeightNotParentPlusOne {
                    parent_height: parent_meta.height,
                    child_height: block.header.height,
                });
            }

            let tip_changed_here = self.maybe_set_tip(id, block.header.height)?;
            self.connect_descendants_from(id)?;

            let outcome = if tip_changed_here {
                IngestOutcome::NewTip
            } else {
                IngestOutcome::StoredConnected
            };
            return Ok((id, outcome));
        }

        // CASE: header chưa có
        self.store.put_header(id, &block.header)?;
        self.store.put_block(id, &block)?;
        self.store.put_block_meta(
            id,
            BlockMeta {
                parent: block.header.parent,
                height: block.header.height,
            },
        )?;
        self.store.add_child(block.header.parent, id)?;

        if !self.store.has_header(block.header.parent)? {
            return Ok((id, IngestOutcome::StoredOrphan));
        }

        let parent_hdr = self.store.get_header(block.header.parent)?;
        let parent_meta = self.ensure_block_meta_from_header(block.header.parent, &parent_hdr)?;
        let expect_h = Height(parent_meta.height.0.saturating_add(1));
        if block.header.height != expect_h {
            return Err(ChainStateError::HeightNotParentPlusOne {
                parent_height: parent_meta.height,
                child_height: block.header.height,
            });
        }

        let tip_changed_here = self.maybe_set_tip(id, block.header.height)?;
        self.connect_descendants_from(id)?;

        let outcome = if tip_changed_here {
            IngestOutcome::NewTip
        } else {
            IngestOutcome::StoredConnected
        };
        Ok((id, outcome))
    }

    pub fn ingest_header(&mut self, header: BlockHeader) -> Result<(Hash256, HeaderIngestOutcome)> {
        if !pow_valid(&header) {
            return Err(ChainStateError::InvalidPow);
        }

        let id = header_id(&header);

        if header.height == Height(0) {
            if id != self.meta.genesis_id {
                return Err(ChainStateError::GenesisIdMismatch {
                    expected: self.meta.genesis_id,
                    got: id,
                });
            }
            return Ok((id, HeaderIngestOutcome::AlreadyKnown));
        }

        if self.store.has_header(id)? {
            return Ok((id, HeaderIngestOutcome::AlreadyKnown));
        }

        self.store.put_header(id, &header)?;
        self.store.put_block_meta(
            id,
            BlockMeta {
                parent: header.parent,
                height: header.height,
            },
        )?;
        self.store.add_child(header.parent, id)?;

        if !self.store.has_header(header.parent)? {
            return Ok((id, HeaderIngestOutcome::StoredOrphan));
        }

        let ph = self.store.get_header(header.parent)?;
        let pm = self.ensure_block_meta_from_header(header.parent, &ph)?;
        let expect_h = Height(pm.height.0.saturating_add(1));
        if header.height != expect_h {
            return Err(ChainStateError::HeightNotParentPlusOne {
                parent_height: pm.height,
                child_height: header.height,
            });
        }

        Ok((id, HeaderIngestOutcome::StoredConnected))
    }

    pub fn validate_best_chain(&self) -> Result<()> {
        let mut cur = self.tip.hash;

        loop {
            let hdr = self.must_header(cur)?;
            let _blk = self.must_block(cur)?;
            let meta = self
                .store
                .get_block_meta(cur)?
                .ok_or(ChainStateError::MissingBlockMeta { id: cur })?;

            if meta.parent != hdr.parent || meta.height != hdr.height {
                return Err(ChainStateError::MissingBlockMeta { id: cur });
            }

            if !pow_valid(&hdr) {
                return Err(ChainStateError::InvalidPow);
            }

            let blk = self.store.get_block(cur)?;
            crate::block_builder::verify_block_merkle(&blk)?;

            if hdr.height == Height(0) {
                if cur != self.meta.genesis_id {
                    return Err(ChainStateError::GenesisIdMismatch {
                        expected: self.meta.genesis_id,
                        got: cur,
                    });
                }
                break;
            }

            let p = hdr.parent;
            let ph = self.must_header(p)?;
            let pm = self.ensure_block_meta_from_header(p, &ph)?;
            let expect_h = Height(pm.height.0.saturating_add(1));
            if hdr.height != expect_h {
                return Err(ChainStateError::HeightNotParentPlusOne {
                    parent_height: pm.height,
                    child_height: hdr.height,
                });
            }

            cur = p;
        }

        Ok(())
    }

    pub fn mine_and_append_one(
        &mut self,
        mempool: &mut crate::mempool::Mempool,
        timestamp_utc: i64,
        pow_difficulty_bits: u32,
    ) -> Result<Hash256> {
        let parent = self.tip.hash;
        let height = Height(self.tip.height.0.saturating_add(1));

        let mined = crate::miner::mine_block_from_mempool(
            mempool,
            parent,
            height,
            timestamp_utc,
            pow_difficulty_bits,
        )?;

        let (id, _out) = self.ingest_block(mined)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_crypto::merkle::merkle_root_txids;
    use egg_db::store::BlockStore;
    use egg_db::store::DbChainStore;
    use egg_db::MemKv;
    use egg_types::{ChainParams, GenesisSpec};

    fn mk_spec(ts: i64) -> ChainSpec {
        ChainSpec {
            spec_version: 1,
            chain: ChainParams {
                chain_name: "EGG-MAINNET".to_string(),
                chain_id: 1,
            },
            genesis: GenesisSpec {
                timestamp_utc: ts,
                pow_difficulty_bits: 0,
                nonce: 0,
            },
        }
    }

    fn mk_empty_block(parent: Hash256, height: Height, nonce: u64) -> Block {
        let merkle_root = merkle_root_txids(&[]);
        let header = BlockHeader {
            parent,
            height,
            timestamp_utc: 1_700_000_000,
            nonce,
            merkle_root,
            pow_difficulty_bits: 0,
        };
        Block { header, txs: vec![] }
    }

    #[test]
    fn fork_choice_tie_breaks_by_smaller_hash() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);

        let mut st = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();
        assert_eq!(st.tip.height, Height(0));
        let g = st.tip.hash;

        let b1 = mk_empty_block(g, Height(1), 1);
        let b2 = mk_empty_block(g, Height(1), 2);

        let id1 = header_id(&b1.header);
        let id2 = header_id(&b2.header);

        let _ = st.ingest_block(b1).unwrap();
        let _ = st.ingest_block(b2).unwrap();

        let expected = if id1.0 < id2.0 { id1 } else { id2 };
        assert_eq!(st.tip.height, Height(1));
        assert_eq!(st.tip.hash, expected);

        assert_eq!(st.canon_hash(Height(1)).unwrap(), Some(expected));

        st.validate_best_chain().unwrap();
    }

    #[test]
    fn orphan_connect_and_reorg_to_longer_chain() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);

        let mut st = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();
        let g = st.tip.hash;

        let a1 = mk_empty_block(g, Height(1), 11);
        let a1id = header_id(&a1.header);
        st.ingest_block(a1).unwrap();

        let a2 = mk_empty_block(a1id, Height(2), 12);
        let a2id = header_id(&a2.header);
        st.ingest_block(a2).unwrap();

        assert_eq!(st.tip.height, Height(2));
        assert_eq!(st.tip.hash, a2id);

        let b1 = mk_empty_block(g, Height(1), 21);
        let b1id = header_id(&b1.header);

        let b2 = mk_empty_block(b1id, Height(2), 22);
        let b2id = header_id(&b2.header);

        let (id_b2, out_b2) = st.ingest_block(b2).unwrap();
        assert_eq!(id_b2, b2id);
        assert_eq!(out_b2, IngestOutcome::StoredOrphan);

        let (id_b1, _out_b1) = st.ingest_block(b1).unwrap();
        assert_eq!(id_b1, b1id);

        let b3 = mk_empty_block(b2id, Height(3), 23);
        let b3id = header_id(&b3.header);
        let _ = st.ingest_block(b3).unwrap();

        assert_eq!(st.tip.height, Height(3));
        assert_eq!(st.tip.hash, b3id);

        assert_eq!(st.canon_hash(Height(1)).unwrap(), Some(b1id));
        assert_eq!(st.canon_hash(Height(2)).unwrap(), Some(b2id));
        assert_eq!(st.canon_hash(Height(3)).unwrap(), Some(b3id));

        st.validate_best_chain().unwrap();
    }

    #[test]
    fn ingest_header_stores_orphan_then_parent_connects() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);
        let mut st = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();
        let g = st.tip.hash;

        let h1 = BlockHeader {
            parent: g,
            height: Height(1),
            timestamp_utc: 1_700_000_000,
            nonce: 100,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 0,
        };
        let h1id = header_id(&h1);

        let h2 = BlockHeader {
            parent: h1id,
            height: Height(2),
            timestamp_utc: 1_700_000_000,
            nonce: 101,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 0,
        };
        let h2id = header_id(&h2);

        let (id2, o2) = st.ingest_header(h2).unwrap();
        assert_eq!(id2, h2id);
        assert_eq!(o2, HeaderIngestOutcome::StoredOrphan);
        assert!(store.has_header(h2id).unwrap());

        let (id1, o1) = st.ingest_header(h1).unwrap();
        assert_eq!(id1, h1id);
        assert_eq!(o1, HeaderIngestOutcome::StoredConnected);
        assert!(store.has_header(h1id).unwrap());

        assert!(store.get_block_meta(h1id).unwrap().is_some());
        assert!(store.get_block_meta(h2id).unwrap().is_some());

        let ch = store.get_children(h1id).unwrap();
        assert!(ch.iter().any(|x| *x == h2id));
    }

    #[test]
    fn get_headers_after_returns_canonical_sequence() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);
        let mut st = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();

        let g = st.tip.hash;
        let b1 = mk_empty_block(g, Height(1), 1);
        let b1id = header_id(&b1.header);
        st.ingest_block(b1).unwrap();

        let b2 = mk_empty_block(b1id, Height(2), 2);
        let _b2id = header_id(&b2.header);
        st.ingest_block(b2).unwrap();

        let hs = st.get_headers_after(g, 10).unwrap();
        assert_eq!(hs.len(), 2);
        assert_eq!(hs[0].height, Height(1));
        assert_eq!(hs[1].height, Height(2));

        let hs2 = st.get_headers_after(b1id, 10).unwrap();
        assert_eq!(hs2.len(), 1);
        assert_eq!(hs2[0].height, Height(2));

        let hs3 = st.get_headers_after(Hash256([9u8; 32]), 10).unwrap();
        assert!(hs3.is_empty());
    }

    #[test]
    fn ingest_block_when_header_preexists_puts_block_and_connects() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);
        let mut st = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();

        let g = st.tip.hash;

        // ingest header trước
        let h1 = BlockHeader {
            parent: g,
            height: Height(1),
            timestamp_utc: 1_700_000_000,
            nonce: 9,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 0,
        };
        let h1id = header_id(&h1);
        st.ingest_header(h1.clone()).unwrap();
        assert!(store.has_header(h1id).unwrap());
        assert!(!store.has_block(h1id).unwrap());

        // giờ ingest block cùng header
        let b1 = Block { header: h1, txs: vec![] };
        let (_id, out) = st.ingest_block(b1).unwrap();
        assert!(store.has_block(h1id).unwrap());
        assert!(matches!(out, IngestOutcome::NewTip | IngestOutcome::StoredConnected));
        assert_eq!(st.tip.height, Height(1));
        assert_eq!(st.tip.hash, h1id);
    }
}
