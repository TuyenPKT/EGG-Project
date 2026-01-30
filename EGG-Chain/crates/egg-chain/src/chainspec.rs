#![forbid(unsafe_code)]

use std::path::Path;

use egg_types::{Block, BlockHeader, ChainSpec, Hash256, Height};
use thiserror::Error;

use crate::{header_id, pow_valid};

#[derive(Debug, Error)]
pub enum ChainSpecError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml decode error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("chainspec invalid: {0}")]
    Invalid(&'static str),
}

pub type Result<T> = std::result::Result<T, ChainSpecError>;

pub fn validate_chainspec(spec: &ChainSpec) -> Result<()> {
    if spec.spec_version == 0 {
        return Err(ChainSpecError::Invalid("spec_version must be > 0"));
    }
    if spec.chain.chain_name.trim().is_empty() {
        return Err(ChainSpecError::Invalid("chain.chain_name must be non-empty"));
    }
    if spec.genesis.timestamp_utc <= 0 {
        return Err(ChainSpecError::Invalid(
            "genesis.timestamp_utc must be > 0 (UTC seconds)",
        ));
    }
    Ok(())
}

pub fn save_chainspec_to_path<P: AsRef<Path>>(path: P, spec: &ChainSpec) -> Result<()> {
    validate_chainspec(spec)?;
    let s = toml::to_string(spec)?;
    std::fs::write(path, s)?;
    Ok(())
}

pub fn load_chainspec_from_path<P: AsRef<Path>>(path: P) -> Result<ChainSpec> {
    let s = std::fs::read_to_string(path)?;
    let spec: ChainSpec = toml::from_str(&s)?;
    validate_chainspec(&spec)?;
    Ok(spec)
}

/// Mainnet_Official_Start = genesis block.
/// Genesis header luôn có:
/// - parent = 0
/// - height = 0
/// - merkle_root = 0 (chưa có tx trong genesis ở bước này)
pub fn genesis_header(spec: &ChainSpec) -> Result<BlockHeader> {
    validate_chainspec(spec)?;

    Ok(BlockHeader {
        parent: Hash256::zero(),
        height: Height(0),
        timestamp_utc: spec.genesis.timestamp_utc,
        nonce: spec.genesis.nonce,
        merkle_root: Hash256::zero(),
        pow_difficulty_bits: spec.genesis.pow_difficulty_bits,
    })
}

/// Genesis block: header + tx list rỗng (deterministic).
pub fn genesis_block(spec: &ChainSpec) -> Result<Block> {
    let header = genesis_header(spec)?;
    Ok(Block { header, txs: vec![] })
}

pub fn genesis_id(spec: &ChainSpec) -> Result<Hash256> {
    let header = genesis_header(spec)?;
    Ok(header_id(&header))
}

/// Kiểm tra genesis PoW hợp lệ theo pow_difficulty_bits trong header.
/// Nếu difficulty_bits > 0, chainspec phải cung cấp nonce phù hợp.
pub fn genesis_pow_valid(spec: &ChainSpec) -> Result<bool> {
    let header = genesis_header(spec)?;
    Ok(pow_valid(&header))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_db::store::{BlockStore, ChainStore, ChainTip, DbChainStore};
    use egg_db::MemKv;
    use egg_types::{ChainParams, GenesisSpec};

    fn mk_spec() -> ChainSpec {
        ChainSpec {
            spec_version: 1,
            chain: ChainParams {
                chain_name: "EGG-MAINNET".to_string(),
                chain_id: 1,
            },
            genesis: GenesisSpec {
                timestamp_utc: 1_700_000_000,
                pow_difficulty_bits: 0,
                nonce: 0,
            },
        }
    }

    #[test]
    fn load_save_roundtrip_toml() {
        let spec = mk_spec();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chainspec.toml");

        save_chainspec_to_path(&path, &spec).unwrap();
        let back = load_chainspec_from_path(&path).unwrap();

        assert_eq!(spec, back);
    }

    #[test]
    fn genesis_is_mainnet_official_start() {
        let spec = mk_spec();
        let hdr = genesis_header(&spec).unwrap();
        assert_eq!(hdr.timestamp_utc, spec.genesis.timestamp_utc);
        assert_eq!(hdr.height.0, 0);
        assert_eq!(hdr.parent, Hash256::zero());
    }

    #[test]
    fn genesis_id_is_stable() {
        let spec = mk_spec();
        let a = genesis_id(&spec).unwrap();
        let b = genesis_id(&spec).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, Hash256::zero());
    }

    #[test]
    fn genesis_block_is_deterministic_and_decodable() {
        let spec = mk_spec();
        let blk = genesis_block(&spec).unwrap();

        let enc = egg_types::canonical::encode_block(&blk);
        let dec = egg_types::canonical::decode_block(&enc).unwrap();
        assert_eq!(blk, dec);
    }

    #[test]
    fn genesis_pow_valid_when_difficulty_zero() {
        let spec = mk_spec();
        assert_eq!(genesis_pow_valid(&spec).unwrap(), true);
    }

    #[test]
    fn validate_rejects_bad_spec() {
        let mut spec = mk_spec();
        spec.spec_version = 0;
        assert!(validate_chainspec(&spec).is_err());
    }

    #[test]
    fn store_and_load_genesis_via_chainstore() {
        let spec = mk_spec();
        let blk = genesis_block(&spec).unwrap();
        let gid = genesis_id(&spec).unwrap();

        let kv = MemKv::new();
        let store = DbChainStore::new(kv);

        // Lưu genesis header + block theo gid, và đặt tip = genesis.
        store.put_header(gid, &blk.header).unwrap();
        store.put_block(gid, &blk).unwrap();
        store
            .set_tip(ChainTip {
                height: blk.header.height,
                hash: gid,
            })
            .unwrap();

        // Đọc lại và so sánh bất biến.
        let hdr_back = store.get_header(gid).unwrap();
        let blk_back = store.get_block(gid).unwrap();
        let tip_back = store.get_tip().unwrap().expect("tip exists");

        assert_eq!(blk.header, hdr_back);
        assert_eq!(blk, blk_back);
        assert_eq!(tip_back.height, Height(0));
        assert_eq!(tip_back.hash, gid);
    }
}
