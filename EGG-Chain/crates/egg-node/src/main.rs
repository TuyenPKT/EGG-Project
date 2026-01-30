#![forbid(unsafe_code)]

use egg_chain::state::ChainState;
use egg_db::store::DbChainStore;
use egg_db::MemKv;
use egg_types::{ChainParams, ChainSpec, GenesisSpec};

fn main() {
    if let Err(e) = run() {
        eprintln!("egg-node error: {e}");
        std::process::exit(1);
    }
    println!("egg-node: boot ok");
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // BƯỚC 4: khởi tạo chainstate theo spec cố định để đảm bảo init DB/genesis/tip hoạt động.
    // (BƯỚC sau sẽ thêm cấu hình/đường dẫn persistent store và nạp spec từ file.)
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

    let kv = MemKv::new();
    let store = DbChainStore::new(kv);

    let state = ChainState::open_or_init(store, spec)?;
    state.verify_genesis_matches_spec()?;

    Ok(())
}
