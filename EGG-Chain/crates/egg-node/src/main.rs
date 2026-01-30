#![forbid(unsafe_code)]

use std::path::PathBuf;

use egg_chain::state::ChainState;
use egg_db::store::DbChainStore;
use egg_db::SledKv;
use egg_types::{ChainParams, ChainSpec, GenesisSpec};

fn main() {
    if let Err(e) = run() {
        eprintln!("egg-node error: {e}");
        std::process::exit(1);
    }
    println!("egg-node: boot ok");
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // BƯỚC 5: DB bền vững mặc định (sled) trên disk.
    // Đường dẫn mặc định: EGG-Project/EGG-Chain/data/egg-node (tính theo working dir = EGG-Chain).
    let db_dir: PathBuf = PathBuf::from("data").join("egg-node");
    std::fs::create_dir_all(&db_dir)?;

    let spec = ChainSpec {
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
    };

    let kv = SledKv::open(&db_dir)?;
    let store = DbChainStore::new(kv);

    let state = ChainState::open_or_init(store, spec)?;
    state.verify_genesis_matches_spec()?;

    Ok(())
}
