#![forbid(unsafe_code)]

use egg_db::store::{ChainStore, ChainTip, StoreError};
use egg_types::{ChainSpec, Hash256, Height};
use thiserror::Error;

use crate::chainspec::{genesis_id, genesis_header, validate_chainspec, ChainSpecError};

#[derive(Debug, Error)]
pub enum ChainStateError {
    #[error("chainspec error: {0}")]
    Spec(#[from] ChainSpecError),

    #[error("store error: {0}")]
    Store(#[from] StoreError),

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
}

pub type Result<T> = std::result::Result<T, ChainStateError>;

#[derive(Clone)]
pub struct ChainState<S: ChainStore + Clone> {
    pub spec: ChainSpec,
    pub tip: ChainTip,
    store: S,
}

impl<S: ChainStore + Clone> ChainState<S> {
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Mở chainstate từ store.
    /// - Nếu chưa có tip: init genesis (header+block+tip)
    /// - Nếu đã có tip: verify tối thiểu
    ///
    /// Idempotent: gọi lặp lại với cùng store + spec sẽ không tạo lại genesis.
    pub fn open_or_init(store: S, spec: ChainSpec) -> Result<Self> {
        validate_chainspec(&spec)?;

        match store.get_tip()? {
            Some(tip) => {
                // Quy tắc ưu tiên: nếu height=0 thì tip BẮT BUỘC là genesis theo spec.
                // Điều này cần trả lỗi rõ ràng trước khi check dữ liệu block/header có tồn tại không.
                if tip.height == Height(0) {
                    let gid = genesis_id(&spec)?;
                    if tip.hash != gid {
                        return Err(ChainStateError::TipNotGenesisAtHeight0);
                    }
                }

                // Verify tối thiểu: tip phải có header+block
                if !store.has_header(tip.hash)? {
                    return Err(ChainStateError::TipHeaderMissing);
                }
                if !store.has_block(tip.hash)? {
                    return Err(ChainStateError::TipBlockMissing);
                }

                Ok(Self { spec, tip, store })
            }
            None => {
                // Init genesis
                let hdr = genesis_header(&spec)?;
                let gid = genesis_id(&spec)?;

                // Consistency check: header_id(genesis_header) phải đúng gid
                let computed = crate::header_id(&hdr);
                if computed != gid {
                    return Err(ChainStateError::GenesisIdMismatch {
                        expected: gid,
                        got: computed,
                    });
                }

                // Genesis block deterministic: tx list rỗng.
                let blk = egg_types::Block {
                    header: hdr.clone(),
                    txs: vec![],
                };

                store.put_header(gid, &hdr)?;
                store.put_block(gid, &blk)?;
                let tip = ChainTip {
                    height: Height(0),
                    hash: gid,
                };
                store.set_tip(tip)?;

                Ok(Self { spec, tip, store })
            }
        }
    }

    /// Verify genesis đã lưu trong store khớp spec hiện tại.
    pub fn verify_genesis_matches_spec(&self) -> Result<()> {
        let gid = genesis_id(&self.spec)?;
        let hdr_expected = genesis_header(&self.spec)?;
        let hdr_stored = self.store.get_header(gid)?;
        if hdr_stored != hdr_expected {
            return Err(ChainStateError::GenesisHeaderMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_db::store::{BlockStore, DbChainStore};
    use egg_db::{MemKv, SledKv};
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

    #[test]
    fn open_or_init_is_idempotent_in_memkv() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);

        let st1 = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();
        let st2 = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();

        assert_eq!(st1.tip, st2.tip);

        let gid = genesis_id(&spec).unwrap();
        let hdr1 = store.get_header(gid).unwrap();
        let blk1 = store.get_block(gid).unwrap();

        let hdr2 = store.get_header(gid).unwrap();
        let blk2 = store.get_block(gid).unwrap();

        assert_eq!(hdr1, hdr2);
        assert_eq!(blk1, blk2);
    }

    #[test]
    fn open_or_init_rejects_tip_height0_not_genesis() {
        let kv = MemKv::new();
        let store = DbChainStore::new(kv);
        let spec = mk_spec(1_700_000_000);

        let _ = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();

        let bad_tip = egg_db::store::ChainTip {
            height: Height(0),
            hash: Hash256([7u8; 32]),
        };
        store.set_tip(bad_tip).unwrap();

        let err = match ChainState::open_or_init(store.clone(), spec.clone()) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        assert!(matches!(err, ChainStateError::TipNotGenesisAtHeight0));
    }

    #[test]
    fn persistent_restart_keeps_tip_and_genesis() {
        let spec = mk_spec(1_700_000_000);
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sled-db");

        // First start: init genesis + tip
        {
            let kv = SledKv::open(&db_path).unwrap();
            let store = DbChainStore::new(kv);
            let st1 = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();
            st1.verify_genesis_matches_spec().unwrap();
            assert_eq!(st1.tip.height, Height(0));
        }

        // Restart: reopen same path, must load existing tip and not re-init
        {
            let kv = SledKv::open(&db_path).unwrap();
            let store = DbChainStore::new(kv);
            let st2 = ChainState::open_or_init(store.clone(), spec.clone()).unwrap();
            st2.verify_genesis_matches_spec().unwrap();

            let gid = genesis_id(&spec).unwrap();
            assert_eq!(st2.tip.height, Height(0));
            assert_eq!(st2.tip.hash, gid);

            assert!(store.has_header(gid).unwrap());
            assert!(store.has_block(gid).unwrap());
        }
    }

    #[test]
    fn persistent_restart_rejects_changed_chainspec() {
        let spec_a = mk_spec(1_700_000_000);
        let spec_b = mk_spec(1_700_000_001);

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sled-db");

        // Start with spec A
        {
            let kv = SledKv::open(&db_path).unwrap();
            let store = DbChainStore::new(kv);
            let st = ChainState::open_or_init(store.clone(), spec_a.clone()).unwrap();
            st.verify_genesis_matches_spec().unwrap();
            assert_eq!(st.tip.height, Height(0));
        }

        // Restart with spec B must be rejected
        {
            let kv = SledKv::open(&db_path).unwrap();
            let store = DbChainStore::new(kv);
            let err = match ChainState::open_or_init(store.clone(), spec_b.clone()) {
                Ok(_) => panic!("expected error"),
                Err(e) => e,
            };
            assert!(matches!(err, ChainStateError::TipNotGenesisAtHeight0));
        }
    }
}
