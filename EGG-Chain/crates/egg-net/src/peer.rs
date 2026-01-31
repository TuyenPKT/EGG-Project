#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use egg_crypto::hash_header;
use egg_types::{BlockHeader, Hash256};

use crate::protocol::{Message, Tip};

const MAX_NOTFOUND_PER_ID: u8 = 2;
const MAX_DISTINCT_NOTFOUND_IDS: usize = 16;

// ---- Penalty / Decay / Ban threshold ----
const PENALTY_BAN_THRESHOLD: i32 = 100;

// decay: mỗi PENALTY_DECAY_EVERY giây giảm PENALTY_DECAY_STEP điểm (lazy decay)
const PENALTY_DECAY_EVERY: Duration = Duration::from_secs(10);
const PENALTY_DECAY_STEP: i32 = 10;

// penalties
const PENALTY_UNSOLICITED_REPLY: i32 = 55;
const PENALTY_REPLY_WITHOUT_KNOWN_HEADER: i32 = 65;
const PENALTY_BLOCK_ID_MISMATCH: i32 = 70;
const PENALTY_BLOCK_NOTFOUND: i32 = 5;
const PENALTY_TOO_MANY_NOTFOUND_PER_ID: i32 = 25;
const PENALTY_TOO_MANY_DISTINCT_NOTFOUND: i32 = 40;
const PENALTY_TIMEOUT: i32 = 8;

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

    // ban state
    banned: Option<String>,

    // hardening sets
    known_header_ids: HashSet<Hash256>,
    inflight_blocks: HashSet<Hash256>,

    // notfound tracking (để tính pattern)
    notfound_by_id: HashMap<Hash256, u8>,
    notfound_distinct_ids: HashSet<Hash256>,

    // penalty
    penalty_score: i32,
    penalty_last_decay: Instant,
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

            penalty_score: 0,
            penalty_last_decay: Instant::now(),
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

    pub fn penalty_score(&self) -> i32 {
        self.penalty_score
    }

    // --- telemetry getters (BƯỚC 29) ---
    pub fn distinct_notfound_count(&self) -> usize {
        self.notfound_distinct_ids.len()
    }

    pub fn inflight_blocks_count(&self) -> usize {
        self.inflight_blocks.len()
    }

    fn ban(&mut self, reason: impl Into<String>) {
        if self.banned.is_none() {
            self.banned = Some(reason.into());
        }
    }

    fn apply_decay(&mut self, now: Instant) {
        let Some(elapsed) = now.checked_duration_since(self.penalty_last_decay) else {
            self.penalty_last_decay = now;
            return;
        };

        let every = PENALTY_DECAY_EVERY.as_secs();
        if every == 0 {
            return;
        }

        let steps = elapsed.as_secs() / every;
        if steps == 0 {
            return;
        }

        let dec = (steps as i32).saturating_mul(PENALTY_DECAY_STEP);
        self.penalty_score = (self.penalty_score - dec).max(0);
        self.penalty_last_decay = now;
    }

    fn add_penalty(&mut self, now: Instant, points: i32, why: &str) {
        if self.is_banned() {
            return;
        }

        self.apply_decay(now);
        self.penalty_score = self.penalty_score.saturating_add(points);

        if self.penalty_score >= PENALTY_BAN_THRESHOLD {
            self.ban(format!(
                "penalty threshold exceeded: score={} reason={}",
                self.penalty_score, why
            ));
        }
    }

    pub fn note_timeout(&mut self) {
        self.add_penalty(Instant::now(), PENALTY_TIMEOUT, "timeout");
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

    fn hardening_on_block_reply(&mut self, now: Instant, id: Hash256) -> bool {
        // 1) unsolicited reply
        if !self.inflight_blocks.remove(&id) {
            self.add_penalty(now, PENALTY_UNSOLICITED_REPLY, "unsolicited block reply");
            return false;
        }

        // 2) reply nhưng chưa biết header
        if !self.known_header_ids.contains(&id) {
            self.add_penalty(
                now,
                PENALTY_REPLY_WITHOUT_KNOWN_HEADER,
                "block reply without known header",
            );
            return false;
        }

        true
    }

    fn hardening_on_notfound(&mut self, now: Instant, id: Hash256) {
        // base penalty
        self.add_penalty(now, PENALTY_BLOCK_NOTFOUND, "BlockNotFound");

        // không giữ mutable borrow từ entry() qua các lần gọi self.add_penalty(...)
        let count: u8 = {
            let e = self.notfound_by_id.entry(id).or_insert(0);
            *e = e.saturating_add(1);
            *e
        };

        // per-id escalation (không ban ngay; chỉ cộng điểm)
        if count > MAX_NOTFOUND_PER_ID {
            self.add_penalty(
                now,
                PENALTY_TOO_MANY_NOTFOUND_PER_ID,
                "too many BlockNotFound per id",
            );
        }

        // distinct tracking (pattern)
        if count == 1 {
            let prev = self.notfound_distinct_ids.len();
            self.notfound_distinct_ids.insert(id);

            if prev <= MAX_DISTINCT_NOTFOUND_IDS
                && self.notfound_distinct_ids.len() > MAX_DISTINCT_NOTFOUND_IDS
            {
                self.add_penalty(
                    now,
                    PENALTY_TOO_MANY_DISTINCT_NOTFOUND,
                    "too many distinct BlockNotFound ids",
                );
            }
        }
    }

    fn hardening_on_found(&mut self, id: Hash256) {
        self.notfound_by_id.remove(&id);
        self.notfound_distinct_ids.remove(&id);
    }

    pub fn on_message(&mut self, msg: Message) -> Vec<Message> {
        self.on_message_at(msg, Instant::now())
    }

    fn on_message_at(&mut self, msg: Message, now: Instant) -> Vec<Message> {
        if self.is_banned() {
            return vec![];
        }

        // lazy decay trước khi xử lý msg
        self.apply_decay(now);

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
                if !self.hardening_on_block_reply(now, id) {
                    return vec![];
                }

                let hid = hash_header(&block.header);
                if hid != id {
                    self.add_penalty(now, PENALTY_BLOCK_ID_MISMATCH, "BlockFound id mismatch");
                    return vec![];
                }

                self.hardening_on_found(id);
                vec![]
            }

            Message::BlockNotFound { id } => {
                if !self.hardening_on_block_reply(now, id) {
                    return vec![];
                }

                self.hardening_on_notfound(now, id);
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
    fn penalty_ban_requires_threshold_not_immediate() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let t0 = Instant::now();
        let _ = p.on_message_at(mk_ack(), t0);
        assert!(p.is_ready());

        let h = hdr(Hash256::zero(), 1, 1);
        let id = hash_header(&h);
        let blk = Block { header: h, txs: vec![] };

        let _ = p.on_message_at(
            Message::BlockFound { id, block: blk.clone() },
            t0 + Duration::from_secs(1),
        );
        assert!(!p.is_banned());
        assert!(p.penalty_score() > 0);

        let _ = p.on_message_at(
            Message::BlockFound { id, block: blk },
            t0 + Duration::from_secs(2),
        );
        assert!(p.is_banned());
        assert!(p.ban_reason().unwrap().contains("threshold"));
    }

    #[test]
    fn penalty_decays_over_time() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let t0 = Instant::now();
        let _ = p.on_message_at(mk_ack(), t0);
        assert!(p.is_ready());

        let h = hdr(Hash256::zero(), 1, 1);
        let id = hash_header(&h);
        let blk = Block { header: h, txs: vec![] };

        let _ = p.on_message_at(
            Message::BlockFound { id, block: blk },
            t0 + Duration::from_secs(1),
        );
        assert!(p.penalty_score() > 0);

        let _ = p.on_message_at(Message::Pong { nonce: 1 }, t0 + Duration::from_secs(61));
        assert_eq!(p.penalty_score(), 0);
        assert!(!p.is_banned());
    }

    #[test]
    fn ban_after_too_many_distinct_notfound_ids_via_penalty_threshold() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let t0 = Instant::now();
        let _ = p.on_message_at(mk_ack(), t0);
        assert!(p.is_ready());

        let mut headers = Vec::new();
        for i in 1..=(MAX_DISTINCT_NOTFOUND_IDS as u64 + 1) {
            headers.push(hdr(Hash256::zero(), i, 10_000 + i));
        }
        let _ = p.on_message_at(
            Message::Headers { headers: headers.clone() },
            t0 + Duration::from_secs(1),
        );

        for (idx, h) in headers.into_iter().enumerate() {
            let id = hash_header(&h);

            let _ = p.request_block(id);
            let _ = p.on_message_at(
                Message::BlockNotFound { id },
                t0 + Duration::from_secs(2 + idx as u64),
            );

            if idx < MAX_DISTINCT_NOTFOUND_IDS {
                assert!(!p.is_banned(), "should not be banned yet at idx={}", idx);
            } else {
                assert!(p.is_banned(), "should be banned at idx={}", idx);
                break;
            }
        }
    }

    #[test]
    fn reply_without_known_header_is_penalized_not_immediate_ban() {
        let mut p = PeerMachine::new(Role::Outbound, mk_local());
        let t0 = Instant::now();
        let _ = p.on_message_at(mk_ack(), t0);
        assert!(p.is_ready());

        let id = Hash256([8u8; 32]);

        let _ = p.request_block(id);
        let _ = p.on_message_at(Message::BlockNotFound { id }, t0 + Duration::from_secs(1));
        assert!(!p.is_banned());

        let _ = p.request_block(id);
        let _ = p.on_message_at(Message::BlockNotFound { id }, t0 + Duration::from_secs(2));
        assert!(p.is_banned());
    }
}
