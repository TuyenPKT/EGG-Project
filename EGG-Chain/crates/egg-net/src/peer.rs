#![forbid(unsafe_code)]

use egg_crypto::hash_header;
use egg_types::{BlockHeader, Hash256};

use crate::protocol::{Message, Tip};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Inbound,
    Outbound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeState {
    Init,
    SentHello,
    ReceivedHello,
    Ready,
}

#[derive(Clone, Debug)]
pub struct LocalInfo {
    pub chain_id: u32,
    pub genesis_id: Hash256,
    pub tip: Tip,
    pub node_nonce: u64,
    pub agent: String,
}

#[derive(Clone, Debug)]
pub struct RemoteInfo {
    pub chain_id: u32,
    pub genesis_id: Hash256,
    pub tip: Tip,
    pub node_nonce: u64,
    pub agent: String,
}

#[derive(Clone, Debug)]
pub struct PeerMachine {
    role: Role,
    hs: HandshakeState,
    local: LocalInfo,
    remote: Option<RemoteInfo>,

    // headers-first sync cursor
    sync_enabled: bool,
    sync_cursor_start: Hash256,
    sync_batch_max: u32,
}

impl PeerMachine {
    pub fn new(role: Role, local: LocalInfo) -> Self {
        Self {
            role,
            hs: HandshakeState::Init,
            sync_enabled: false,
            sync_cursor_start: local.tip.hash,
            sync_batch_max: 2000,
            local,
            remote: None,
        }
    }

    pub fn enable_header_sync(mut self, batch_max: u32) -> Self {
        self.sync_enabled = true;
        self.sync_batch_max = batch_max.max(1);
        self.sync_cursor_start = self.local.tip.hash;
        self
    }

    pub fn is_ready(&self) -> bool {
        self.hs == HandshakeState::Ready
    }

    pub fn remote_info(&self) -> Option<&RemoteInfo> {
        self.remote.as_ref()
    }

    pub fn start(&mut self) -> Vec<Message> {
        if self.role == Role::Outbound && self.hs == HandshakeState::Init {
            self.hs = HandshakeState::SentHello;
            return vec![Message::Hello {
                chain_id: self.local.chain_id,
                genesis_id: self.local.genesis_id,
                tip: self.local.tip,
                node_nonce: self.local.node_nonce,
                agent: self.local.agent.clone(),
            }];
        }
        vec![]
    }

    fn make_get_headers(&self, start: Hash256) -> Message {
        Message::GetHeaders {
            start,
            max: self.sync_batch_max,
        }
    }

    fn mark_remote(
        &mut self,
        chain_id: u32,
        genesis_id: Hash256,
        tip: Tip,
        node_nonce: u64,
        agent: String,
    ) {
        self.remote = Some(RemoteInfo {
            chain_id,
            genesis_id,
            tip,
            node_nonce,
            agent,
        });
    }

    fn maybe_sync_kickoff(&mut self) -> Vec<Message> {
        if self.sync_enabled && self.hs == HandshakeState::Ready {
            vec![self.make_get_headers(self.sync_cursor_start)]
        } else {
            vec![]
        }
    }

    pub fn on_message(&mut self, msg: Message) -> Vec<Message> {
        match msg {
            Message::Hello {
                chain_id,
                genesis_id,
                tip,
                node_nonce,
                agent,
            } => {
                self.mark_remote(chain_id, genesis_id, tip, node_nonce, agent);

                match self.hs {
                    HandshakeState::Init => self.hs = HandshakeState::ReceivedHello,
                    HandshakeState::SentHello => {}
                    _ => {}
                }

                let mut out = vec![Message::HelloAck {
                    chain_id: self.local.chain_id,
                    genesis_id: self.local.genesis_id,
                    tip: self.local.tip,
                    node_nonce: self.local.node_nonce,
                    agent: self.local.agent.clone(),
                }];

                self.hs = HandshakeState::Ready;
                out.extend(self.maybe_sync_kickoff());
                out
            }

            Message::HelloAck {
                chain_id,
                genesis_id,
                tip,
                node_nonce,
                agent,
            } => {
                self.mark_remote(chain_id, genesis_id, tip, node_nonce, agent);
                self.hs = HandshakeState::Ready;
                self.maybe_sync_kickoff()
            }

            Message::GetHeaders { start: _, max: _ } => vec![],

            Message::Headers { headers } => {
                if !self.sync_enabled {
                    return vec![];
                }

                if headers.is_empty() {
                    return vec![];
                }

                let last = headers.last().expect("non-empty");
                let last_id = hash_header(last);
                self.sync_cursor_start = last_id;

                vec![self.make_get_headers(self.sync_cursor_start)]
            }

            Message::GetBlock { id: _ } => vec![],
            Message::Block { id: _, block: _ } => vec![],

            Message::Ping { nonce } => vec![Message::Pong { nonce }],
            Message::Pong { nonce: _ } => vec![],
        }
    }
}

pub trait HeaderProvider {
    fn get_headers_after(&self, start: Hash256, max: usize) -> Vec<BlockHeader>;
}

pub fn handle_get_headers<P: HeaderProvider>(p: &P, start: Hash256, max: u32) -> Message {
    let list = p.get_headers_after(start, max as usize);
    Message::Headers { headers: list }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_types::{Hash256, Height};

    fn hdr(parent: Hash256, height: u64, nonce: u64) -> BlockHeader {
        BlockHeader {
            parent,
            height: Height(height),
            timestamp_utc: 1_700_000_000,
            nonce,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 0,
        }
    }

    #[derive(Clone)]
    struct MemProvider {
        headers: Vec<BlockHeader>,
        hashes: Vec<Hash256>,
    }

    impl MemProvider {
        fn new(len: usize) -> Self {
            let mut headers = Vec::with_capacity(len);
            let mut hashes = Vec::with_capacity(len);

            let g_parent = Hash256::zero();
            let g = hdr(g_parent, 0, 1);
            let g_id = hash_header(&g);
            headers.push(g);
            hashes.push(g_id);

            for h in 1..len {
                let parent = hashes[h - 1];
                let x = hdr(parent, h as u64, h as u64);
                let xid = hash_header(&x);
                headers.push(x);
                hashes.push(xid);
            }

            Self { headers, hashes }
        }
    }

    impl HeaderProvider for MemProvider {
        fn get_headers_after(&self, start: Hash256, max: usize) -> Vec<BlockHeader> {
            let mut start_h: Option<usize> = None;
            for (i, h) in self.hashes.iter().enumerate() {
                if *h == start {
                    start_h = Some(i);
                    break;
                }
            }
            let Some(sh) = start_h else { return vec![]; };
            let mut out = Vec::new();
            let mut i = sh + 1;
            while i < self.headers.len() && out.len() < max {
                out.push(self.headers[i].clone());
                i += 1;
            }
            out
        }
    }

    #[test]
    fn handshake_then_headers_sync_requests_next_batch() {
        let chain_id = 1;
        let genesis_id = Hash256([9u8; 32]);

        let provider = MemProvider::new(5);
        let local_tip = Tip {
            height: 0,
            hash: provider.hashes[0],
        };
        let remote_tip = Tip {
            height: 4,
            hash: provider.hashes[4],
        };

        let mut a = PeerMachine::new(
            Role::Outbound,
            LocalInfo {
                chain_id,
                genesis_id,
                tip: local_tip,
                node_nonce: 111,
                agent: "a".to_string(),
            },
        )
        .enable_header_sync(2);

        let mut b = PeerMachine::new(
            Role::Inbound,
            LocalInfo {
                chain_id,
                genesis_id,
                tip: remote_tip,
                node_nonce: 222,
                agent: "b".to_string(),
            },
        );

        let m1 = a.start();
        assert_eq!(m1.len(), 1);

        let out_b = b.on_message(m1[0].clone());
        assert!(out_b.iter().any(|m| matches!(m, Message::HelloAck { .. })));

        let mut out_a = Vec::new();
        for m in out_b {
            out_a.extend(a.on_message(m));
        }
        assert!(a.is_ready());
        assert!(out_a.iter().any(|m| matches!(m, Message::GetHeaders { .. })));

        let mut reply_headers = Vec::new();
        for m in out_a {
            if let Message::GetHeaders { start, max } = m {
                let resp = super::handle_get_headers(&provider, start, max);
                reply_headers.push(resp);
            }
        }
        assert_eq!(reply_headers.len(), 1);
        let Message::Headers { headers } = &reply_headers[0] else {
            panic!("expected headers");
        };
        assert_eq!(headers.len(), 2);

        let out_a2 = a.on_message(reply_headers[0].clone());
        assert_eq!(out_a2.len(), 1);
        let Message::GetHeaders { start, max: _ } = out_a2[0] else {
            panic!("expected GetHeaders");
        };

        let last = headers.last().unwrap();
        let expected_start = hash_header(last);
        assert_eq!(start, expected_start);
    }
}
