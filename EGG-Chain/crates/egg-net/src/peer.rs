#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use egg_crypto::hash_header;
use egg_types::{BlockHeader, Hash256};

use crate::protocol::{Message, Tip};

const MAX_NOTFOUND_PER_ID: u8 = 2;

// Ban nếu peer trả BlockNotFound cho quá nhiều id khác nhau trong cùng session.
// Mục tiêu: bắt pattern “không phục vụ/lying” khi đồng loạt NotFound nhiều block.
const MAX_DISTINCT_NOTFOUND_IDS: usize = 16;

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

    // hardening
    banned: Option<String>,
    known_header_ids: HashSet<Hash256>,
    inflight_blocks: HashSet<Hash256>,

    // notfound tracking
    notfound_by_id: HashMap<Hash256, u8>,
    notfound_distinct_ids: HashSet<Hash256>,
}

impl PeerMachine {
    pub fn new(role: Role, local: LocalInfo) -> Self {
        let mut known = HashSet::new();
        known.insert(local.tip.hash);

        Self {
            role,
            hs: HandshakeState::Init,
            sync_enabled: false,
            sync_cursor_start: local.tip.hash,
            sync_batch_max: 2000,
            local,
            remote: None,
            banned: None,
            known_header_ids: known,
            inflight_blocks: HashSet::new(),
            notfound_by_id: HashMap::new(),
            notfound_distinct_ids: HashSet::new(),
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

    pub fn is_banned(&self) -> bool {
        self.banned.is_some()
    }

    pub fn ban_reason(&self) -> Option<&str> {
        self.banned.as_deref()
    }

    fn ban(&mut self, reason: impl Into<String>) {
        if self.banned.is_none() {
            self.banned = Some(reason.into());
        }
    }

    pub fn start(&mut self) -> Vec<Message> {
        if self.is_banned() {
            return vec![];
        }

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

    pub fn request_block(&mut self, id: Hash256) -> Message {
        self.inflight_blocks.insert(id);
        Message::GetBlock { id }
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

    fn hardening_on_block_reply(&mut self, id: Hash256) -> bool {
        // 1) unsolicited reply -> ban
        if !self.inflight_blocks.remove(&id) {
            self.ban(format!("unsolicited block reply {:?}", id));
            return false;
        }

        // 2) reply nhưng chưa biết header -> ban
        if !self.known_header_ids.contains(&id) {
            self.ban(format!("block reply without known header {:?}", id));
            return false;
        }

        true
    }

    fn hardening_on_notfound(&mut self, id: Hash256) {
        let c = self.notfound_by_id.entry(id).or_insert(0);
        *c = c.saturating_add(1);

        // Ban theo “per-id”
        if *c > MAX_NOTFOUND_PER_ID {
            self.ban(format!("too many BlockNotFound for {:?}", id));
            return;
        }

        // Ban theo “nhiều id khác nhau”
        // Chỉ tính distinct khi đây là lần NotFound đầu tiên của id (c == 1).
        if *c == 1 {
            self.notfound_distinct_ids.insert(id);
            if self.notfound_distinct_ids.len() > MAX_DISTINCT_NOTFOUND_IDS {
                self.ban(format!(
                    "too many distinct BlockNotFound ids: {}",
                    self.notfound_distinct_ids.len()
                ));
            }
        }
    }

    fn hardening_on_found(&mut self, id: Hash256) {
        self.notfound_by_id.remove(&id);
        self.notfound_distinct_ids.remove(&id);
    }

    pub fn on_message(&mut self, msg: Message) -> Vec<Message> {
        if self.is_banned() {
            return vec![];
        }

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
                // hardening: ghi nhận known header ids
                for h in headers.iter() {
                    let id = hash_header(h);
                    self.known_header_ids.insert(id);
                }

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

            Message::BlockFound { id, block } => {
                if !self.hardening_on_block_reply(id) {
                    return vec![];
                }

                // hardening: id phải khớp header hash
                let hid = hash_header(&block.header);
                if hid != id {
                    self.ban(format!("BlockFound id mismatch: expect {:?} got {:?}", id, hid));
                    return vec![];
                }

                self.hardening_on_found(id);
                vec![]
            }

            Message::BlockNotFound { id } => {
                if !self.hardening_on_block_reply(id) {
                    return vec![];
                }

                self.hardening_on_notfound(id);
                vec![]
            }

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
    use egg_types::{Block, Hash256, Height};

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

    fn mk_local() -> LocalInfo {
        let tip = Tip {
            height: 0,
            hash: Hash256::zero(),
        };
        LocalInfo {
            chain_id: 1,
            genesis_id: Hash256([9u8; 32]),
            tip,
            node_nonce: 111,
            agent: "local".to_string(),
        }
    }

    fn mk_ack() -> Message {
        Message::HelloAck {
            chain_id: 1,
            genesis_id: Hash256([9u8; 32]),
            tip: Tip {
                height: 0,
                hash: Hash256::zero(),
            },
            node_nonce: 222,
            agent: "remote".to_string(),
        }
    }

    #[test]
    fn ban_on_unsolicited_block_found() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let _ = p.on_message(mk_ack());
        assert!(p.is_ready());

        let h = hdr(Hash256::zero(), 1, 1);
        let id = hash_header(&h);
        let blk = Block { header: h, txs: vec![] };

        let _ = p.on_message(Message::BlockFound { id, block: blk });

        assert!(p.is_banned());
        assert!(p.ban_reason().unwrap().contains("unsolicited"));
    }

    #[test]
    fn ban_on_block_not_found_without_known_header_even_if_requested() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let _ = p.on_message(mk_ack());
        assert!(p.is_ready());

        let id = Hash256([8u8; 32]);
        let _req = p.request_block(id);

        let _ = p.on_message(Message::BlockNotFound { id });

        assert!(p.is_banned());
        assert!(p.ban_reason().unwrap().contains("without known header"));
    }

    #[test]
    fn ban_after_too_many_notfound_for_same_id() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let _ = p.on_message(mk_ack());
        assert!(p.is_ready());

        let h = hdr(Hash256::zero(), 1, 123);
        let id = hash_header(&h);
        let _ = p.on_message(Message::Headers { headers: vec![h] });

        for i in 0..=MAX_NOTFOUND_PER_ID {
            let _ = p.request_block(id);
            let _ = p.on_message(Message::BlockNotFound { id });
            if i < MAX_NOTFOUND_PER_ID {
                assert!(!p.is_banned());
            }
        }

        assert!(p.is_banned());
        assert!(p.ban_reason().unwrap().contains("too many BlockNotFound"));
    }

    #[test]
    fn ban_after_too_many_distinct_notfound_ids() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let _ = p.on_message(mk_ack());
        assert!(p.is_ready());

        // Tạo nhiều header khác nhau để “known header” cho các id.
        let mut headers = Vec::new();
        for i in 1..=(MAX_DISTINCT_NOTFOUND_IDS as u64 + 1) {
            headers.push(hdr(Hash256::zero(), i, 10_000 + i));
        }
        let _ = p.on_message(Message::Headers { headers: headers.clone() });

        // Mỗi id: request 1 lần rồi trả NotFound 1 lần.
        // Không bị ban cho đến khi vượt ngưỡng distinct.
        for (idx, h) in headers.into_iter().enumerate() {
            let id = hash_header(&h);

            let _ = p.request_block(id);
            let _ = p.on_message(Message::BlockNotFound { id });

            if idx < MAX_DISTINCT_NOTFOUND_IDS {
                assert!(
                    !p.is_banned(),
                    "should not be banned yet at idx={}",
                    idx
                );
            } else {
                assert!(p.is_banned(), "should be banned at idx={}", idx);
                assert!(
                    p.ban_reason().unwrap().contains("distinct BlockNotFound"),
                    "unexpected ban reason: {:?}",
                    p.ban_reason()
                );
                break;
            }
        }
    }

    #[test]
    fn accept_block_found_when_requested_and_header_known() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let _ = p.on_message(mk_ack());
        assert!(p.is_ready());

        let h = hdr(Hash256::zero(), 1, 123);
        let id = hash_header(&h);
        let _ = p.on_message(Message::Headers { headers: vec![h.clone()] });

        let _req = p.request_block(id);
        let blk = Block { header: h, txs: vec![] };
        let _ = p.on_message(Message::BlockFound { id, block: blk });

        assert!(!p.is_banned());
    }
}
