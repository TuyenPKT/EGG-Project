#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use egg_chain::state::ChainState;
use egg_crypto::hash_header;
use egg_db::store::ChainStore;
use egg_net::codec::{decode_frame, encode_frame, FrameError};
use egg_net::peer::{handle_get_headers, HeaderProvider, PeerMachine, Role};
use egg_net::protocol::{Message, Tip};

#[derive(Debug)]
pub enum NodeError {
    Io(std::io::Error),
    Frame(FrameError),
    Chain(String),
    Protocol(String),
}

impl core::fmt::Display for NodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NodeError::Io(e) => write!(f, "io: {}", e),
            NodeError::Frame(e) => write!(f, "frame: {}", e),
            NodeError::Chain(e) => write!(f, "chain: {}", e),
            NodeError::Protocol(e) => write!(f, "protocol: {}", e),
        }
    }
}

impl std::error::Error for NodeError {}

impl From<std::io::Error> for NodeError {
    fn from(value: std::io::Error) -> Self {
        NodeError::Io(value)
    }
}

impl From<FrameError> for NodeError {
    fn from(value: FrameError) -> Self {
        NodeError::Frame(value)
    }
}

pub type Result<T> = std::result::Result<T, NodeError>;

struct FramedTcp {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl FramedTcp {
    fn new(stream: TcpStream) -> Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        Ok(Self {
            stream,
            buf: Vec::with_capacity(64 * 1024),
        })
    }

    fn send(&mut self, msg: &Message) -> Result<()> {
        let frame = encode_frame(msg)?;
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        Ok(())
    }

    fn recv(&mut self) -> Result<Message> {
        loop {
            match decode_frame(&self.buf) {
                Ok((msg, used)) => {
                    self.buf.drain(0..used);
                    return Ok(msg);
                }
                Err(FrameError::UnexpectedEof { .. }) => {}
                Err(e) => return Err(NodeError::Frame(e)),
            }

            let mut tmp = [0u8; 8192];
            let n = self.stream.read(&mut tmp)?;
            if n == 0 {
                return Err(NodeError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "peer closed",
                )));
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }
}

struct ChainProvider<'a, S: ChainStore + Clone> {
    st: &'a ChainState<S>,
}

impl<'a, S: ChainStore + Clone> HeaderProvider for ChainProvider<'a, S> {
    fn get_headers_after(
        &self,
        start: egg_types::Hash256,
        max: usize,
    ) -> Vec<egg_types::BlockHeader> {
        ChainState::<S>::get_headers_after(self.st, start, max).unwrap_or_default()
    }
}

pub fn run_responder_once<S: ChainStore + Clone>(
    listener: TcpListener,
    spec: egg_types::ChainSpec,
    store: S,
) -> Result<()> {
    let (stream, _) = listener.accept()?;
    let mut io = FramedTcp::new(stream)?;

    let st = ChainState::open_or_init(store.clone(), spec).map_err(|e| NodeError::Chain(e.to_string()))?;
    let local_tip = Tip {
        height: st.tip.height.0,
        hash: st.tip.hash,
    };

    let mut peer = PeerMachine::new(
        Role::Inbound,
        egg_net::peer::LocalInfo {
            chain_id: st.meta.chain_id,
            genesis_id: st.meta.genesis_id,
            tip: local_tip,
            node_nonce: 2002,
            agent: "egg-node/responder".to_string(),
        },
    );

    let provider = ChainProvider { st: &st };

    loop {
        let msg = match io.recv() {
            Ok(m) => m,
            Err(NodeError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        };

        let out = peer.on_message(msg.clone());
        for m in out {
            io.send(&m)?;
        }

        if peer.is_banned() {
            return Err(NodeError::Protocol(format!(
                "peer banned: {}",
                peer.ban_reason().unwrap_or("unknown")
            )));
        }

        if peer.is_ready() {
            match msg {
                Message::GetHeaders { start, max } => {
                    let resp = handle_get_headers(&provider, start, max);
                    io.send(&resp)?;
                }
                Message::GetBlock { id } => {
                    let have = egg_db::store::BlockStore::has_block(st.store(), id)
                        .map_err(|e| NodeError::Chain(e.to_string()))?;
                    if !have {
                        io.send(&Message::Block { id, block: None })?;
                    } else {
                        let blk = egg_db::store::BlockStore::get_block(st.store(), id)
                            .map_err(|e| NodeError::Chain(e.to_string()))?;
                        io.send(&Message::Block { id, block: Some(blk) })?;
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

pub fn run_syncer_once<S: ChainStore + Clone>(
    addr: std::net::SocketAddr,
    spec: egg_types::ChainSpec,
    store: S,
    batch_max: u32,
) -> Result<()> {
    let stream = TcpStream::connect(addr)?;
    let mut io = FramedTcp::new(stream)?;

    let mut st = ChainState::open_or_init(store.clone(), spec).map_err(|e| NodeError::Chain(e.to_string()))?;
    let local_tip = Tip {
        height: st.tip.height.0,
        hash: st.tip.hash,
    };

    let mut peer = PeerMachine::new(
        Role::Outbound,
        egg_net::peer::LocalInfo {
            chain_id: st.meta.chain_id,
            genesis_id: st.meta.genesis_id,
            tip: local_tip,
            node_nonce: 1001,
            agent: "egg-node/syncer".to_string(),
        },
    )
    .enable_header_sync(batch_max);

    for m in peer.start() {
        io.send(&m)?;
    }

    // Phase 1: sync headers
    let mut downloaded_ids: Vec<egg_types::Hash256> = Vec::new();
    loop {
        let msg = io.recv()?;

        if let Message::Headers { headers } = &msg {
            for h in headers.iter().cloned() {
                let id = hash_header(&h);
                downloaded_ids.push(id);
                let _ = st.ingest_header(h).map_err(|e| NodeError::Chain(e.to_string()))?;
            }
        }

        let out = peer.on_message(msg.clone());
        for m in out {
            io.send(&m)?;
        }

        if peer.is_banned() {
            return Err(NodeError::Protocol(format!(
                "peer banned: {}",
                peer.ban_reason().unwrap_or("unknown")
            )));
        }

        if matches!(msg, Message::Headers { headers } if headers.is_empty()) {
            break;
        }
    }

    // Phase 2: download blocks (hardening: chỉ nhận Block nếu header đã có)
    for id in downloaded_ids.into_iter() {
        let have = egg_db::store::BlockStore::has_block(st.store(), id)
            .map_err(|e| NodeError::Chain(e.to_string()))?;
        if have {
            continue;
        }

        let req = peer.request_block(id);
        io.send(&req)?;

        loop {
            let msg = io.recv()?;

            let out = peer.on_message(msg.clone());
            for m in out {
                io.send(&m)?;
            }

            if peer.is_banned() {
                return Err(NodeError::Protocol(format!(
                    "peer banned: {}",
                    peer.ban_reason().unwrap_or("unknown")
                )));
            }

            match msg {
                Message::Block { id: rid, block } => {
                    if rid != id {
                        return Err(NodeError::Protocol(format!(
                            "block response id mismatch: expected {:?} got {:?}",
                            id, rid
                        )));
                    }

                    let has_h = egg_db::store::BlockStore::has_header(st.store(), rid)
                        .map_err(|e| NodeError::Chain(e.to_string()))?;
                    if !has_h {
                        return Err(NodeError::Protocol(format!(
                            "received block {:?} but local missing header",
                            rid
                        )));
                    }

                    let Some(b) = block else {
                        return Err(NodeError::Protocol(format!("block not found for {:?}", id)));
                    };

                    let _ = st.ingest_block(b).map_err(|e| NodeError::Chain(e.to_string()))?;
                    break;
                }
                _ => {}
            }
        }
    }

    st.validate_best_chain()
        .map_err(|e| NodeError::Chain(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc;
    use std::thread;

    use egg_crypto::merkle::merkle_root_txids;
    use egg_db::store::DbChainStore;
    use egg_db::MemKv;
    use egg_types::{Block, BlockHeader, ChainParams, ChainSpec, GenesisSpec, Hash256, Height};

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

    fn build_chain_with_blocks(store: DbChainStore<MemKv>, spec: ChainSpec, n_blocks: u64) -> Vec<Hash256> {
        let mut st = ChainState::open_or_init(store.clone(), spec).unwrap();
        let mut hashes = Vec::new();

        hashes.push(st.tip.hash);

        for i in 1..=n_blocks {
            let parent = st.tip.hash;
            let b = mk_empty_block(parent, Height(i), i as u64);
            let (id, _out) = st.ingest_block(b).unwrap();
            hashes.push(id);
        }

        hashes
    }

    #[test]
    fn tcp_two_nodes_sync_headers_and_blocks_to_same_tip() {
        let spec = mk_spec(1_700_000_000);

        let responder_store = DbChainStore::new(MemKv::new());
        let expected_hashes = build_chain_with_blocks(responder_store.clone(), spec.clone(), 25);

        let syncer_store = DbChainStore::new(MemKv::new());
        let _ = ChainState::open_or_init(syncer_store.clone(), spec.clone()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let spec_r = spec.clone();
        let store_r = responder_store.clone();
        let t_responder = thread::spawn(move || {
            run_responder_once(listener, spec_r, store_r).unwrap();
        });

        let (tx_done, rx_done) = mpsc::channel();
        let spec_s = spec.clone();
        let store_s = syncer_store.clone();
        let t_syncer = thread::spawn(move || {
            let r = run_syncer_once(addr, spec_s, store_s, 2000);
            tx_done.send(r.is_ok()).unwrap();
            r.unwrap();
        });

        let ok = rx_done.recv_timeout(Duration::from_secs(15)).unwrap();
        assert!(ok, "syncer did not finish successfully");

        t_syncer.join().unwrap();
        t_responder.join().unwrap();

        for (h, id) in expected_hashes.iter().enumerate() {
            let has_h = egg_db::store::BlockStore::has_header(&syncer_store, *id).unwrap();
            assert!(has_h, "missing header at height {} id={:?}", h, id);

            let has_b = egg_db::store::BlockStore::has_block(&syncer_store, *id).unwrap();
            assert!(has_b, "missing block at height {} id={:?}", h, id);
        }
    }
}
