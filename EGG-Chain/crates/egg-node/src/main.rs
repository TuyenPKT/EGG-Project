#![forbid(unsafe_code)]

use std::path::PathBuf;

use egg_chain::chainspec::load_chainspec_from_path;
use egg_chain::state::ChainState;
use egg_db::store::DbChainStore;
use egg_db::SledKv;

fn main() {
    if let Err(e) = run() {
        eprintln!("egg-node error: {e}");
        std::process::exit(1);
    }
    println!("egg-node: boot ok");
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // BƯỚC 6: nạp ChainSpec từ file cố định trong repo EGG-Chain.
    let chainspec_path: PathBuf = PathBuf::from("config").join("chainspec.toml");
    let spec = load_chainspec_from_path(&chainspec_path)?;

    // DB bền vững mặc định (sled) trên disk.
    // Đường dẫn mặc định: EGG-Chain/data/egg-node
    let db_dir: PathBuf = PathBuf::from("data").join("egg-node");
    std::fs::create_dir_all(&db_dir)?;

    let kv = SledKv::open(&db_dir)?;
    let store = DbChainStore::new(kv);

    let state = ChainState::open_or_init(store, spec)?;
    state.verify_genesis_matches_spec()?;

    Ok(())
}
