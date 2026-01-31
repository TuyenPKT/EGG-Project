#![forbid(unsafe_code)]

use egg_crypto::hash_chainspec;
use egg_db::store::{ChainMeta, ChainStore, ChainTip, StoreError};
use egg_types::{Block, ChainSpec, Hash256, Height};
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

    #[error("chainstore has tip but missing header for tip hash")]
    TipHeaderMissing,

    #[error("chainstore has tip but missing block for tip hash")]
    TipBlockMissing,

    #[error("genesis id mismatch: expected {expected:?}, got {got:?}")]
    GenesisIdMismatch { expected: Hash256, got: Hash256 },

    #[error("genesis header mismatch between spec and stored data")]
    GenesisHeaderMismatch,

    #[error("invalid state: tip height is 0 but tip hash is not genesis")]
    TipNotGenesisAtHeight0,

    #[error("block parent mismatch: expected {expected:?}, got {got:?}")]
    ParentMismatch { expected: Hash256, got: Hash256 },

    #[error("block height mismatch: expected {expected:?}, got {got:?}")]
    HeightMismatch { expected: Height, got: Height },

    #[error("invalid pow for header")]
    InvalidPow,
}

pub type Result<T> = std::result::Result<T, ChainStateError>;

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

    /// Mở chainstate từ store.
    /// - Nếu chưa có tip: init genesis + tip + meta
    /// - Nếu đã có tip: bắt buộc meta tồn tại và khớp spec (kể cả tip > 0)
    pub fn open_or_init(store: S, spec: ChainSpec) -> Result<Self> {
        validate_chainspec(&spec)?;
        let expected = Self::expected_meta(&spec)?;

        match store.get_tip()? {
            Some(tip) => {
                let got = store.get_meta()?.ok_or(ChainStateError::MetaMissing)?;
                if got != expected {
                    return Err(ChainStateError::MetaMismatch { expected, got });
                }

                if !store.has_header(tip.hash)? {
                    return Err(ChainStateError::TipHeaderMissing);
                }
                if !store.has_block(tip.hash)? {
                    return Err(ChainStateError::TipBlockMissing);
                }

                if tip.height == Height(0) && tip.hash != expected.genesis_id {
                    return Err(ChainStateError::TipNotGenesisAtHeight0);
                }

                Ok(Self {
                    spec,
                    tip,
                    meta: got,
                    store,
                })
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

    /// Verify block và commit vào DB:
    /// - parent == tip.hash
    /// - height == tip.height + 1
    /// - merkle_root đúng (và tx.id đúng chuẩn)
    /// - pow_valid(header) == true
    /// Sau đó ghi header+block và update tip.
    pub fn append_block(&mut self, block: Block) -> Result<Hash256> {
        // parent/height
        let expected_parent = self.tip.hash;
        if block.header.parent != expected_parent {
            return Err(ChainStateError::ParentMismatch {
                expected: expected_parent,
                got: block.header.parent,
            });
        }

        let expected_height = Height(self.tip.height.0.saturating_add(1));
        if block.header.height != expected_height {
            return Err(ChainStateError::HeightMismatch {
                expected: expected_height,
                got: block.header.height,
            });
        }

        // merkle + tx.id check
        crate::block_builder::verify_block_merkle(&block)?;

        // pow
        if !pow_valid(&block.header) {
            return Err(ChainStateError::InvalidPow);
        }

        // commit
        let id = header_id(&block.header);
        self.store.put_header(id, &block.header)?;
        self.store.put_block(id, &block)?;
        let new_tip = ChainTip {
            height: block.header.height,
            hash: id,
        };
        self.store.set_tip(new_tip)?;
        self.tip = new_tip;

        Ok(id)
    }

    /// Mine 1 block từ mempool và commit.
    /// Nếu commit fail thì restore tx về mempool (best-effort).
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

        let restore = mined.txs.clone();
        match self.append_block(mined) {
            Ok(id) => Ok(id),
            Err(e) => {
                // best-effort restore
                for tx in restore {
                    let _ = mempool.add_tx(tx);
                }
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_crypto::tx_id_from_payload;
    use egg_db::store::{BlockStore, DbChainStore};
    use egg_db::MemKv;
    use egg_types::{ChainParams, GenesisSpec, Transaction};

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

    fn mk_tx(payload: &[u8]) -> Transaction {
        let id = tx_id_from_payload(payload);
        Transaction {
            id,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn mine_and_append_increases_tip_and_persists_block() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);

        let mut st = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();
        assert_eq!(st.tip.height, Height(0));

        let mut mp = crate::mempool::Mempool::new();
        mp.add_tx(mk_tx(b"a")).unwrap();
        mp.add_tx(mk_tx(b"b")).unwrap();
        mp.add_tx(mk_tx(b"c")).unwrap();

        let new_id = match st.mine_and_append_one(&mut mp, 1_700_000_100, 8) {
            Ok(id) => id,
            Err(e) => panic!("mine_and_append_one failed: {e}"),
        };

        // tip tăng
        assert_eq!(st.tip.height, Height(1));
        assert_eq!(st.tip.hash, new_id);

        // pow hợp lệ
        let hdr = store.get_header(new_id).unwrap();
        assert!(crate::pow_valid(&hdr));

        // block tồn tại và merkle đúng
        let blk = store.get_block(new_id).unwrap();
        crate::block_builder::verify_block_merkle(&blk).unwrap();

        // mempool đã drain
        assert_eq!(mp.len(), 0);

        // DB tip đã update
        let tip = store.get_tip().unwrap().expect("tip exists");
        assert_eq!(tip.height, Height(1));
        assert_eq!(tip.hash, new_id);
    }

    #[test]
    fn open_or_init_creates_genesis_when_empty() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);

        let st = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();
        assert_eq!(st.tip.height, Height(0));

        let gid = genesis_id(&spec).unwrap();
        assert_eq!(st.tip.hash, gid);
        assert!(st.store().has_header(gid).unwrap());
        assert!(st.store().has_block(gid).unwrap());
        st.verify_genesis_matches_spec().unwrap();
    }
}
