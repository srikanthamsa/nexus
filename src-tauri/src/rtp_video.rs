//! Reassembles HEVC access units from RTP packets (RFC 7798, without DONL).
//!
//! Packets are ordered by extended sequence number. An access unit is emitted
//! once every packet from the expected sequence number through the one
//! carrying the marker bit is present. A hole that outlives the loss timeout
//! (long enough for NACK retransmission and FEC repair to fill it) is declared
//! lost, after which only IRAP access units are emitted until the decoder has
//! a fresh reference again.

use bytes::Bytes;
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

const NAL_AGGREGATION: u8 = 48;
const NAL_FRAGMENT: u8 = 49;
const NAL_PACI: u8 = 50;
const START_CODE: [u8; 4] = [0, 0, 0, 1];
const MAX_BUFFERED_PACKETS: usize = 8192;
pub const DEFAULT_LOSS_TIMEOUT: Duration = Duration::from_millis(80);

#[derive(Debug, Clone)]
pub struct RtpVideoPacket {
    pub sequence: u16,
    pub timestamp: u32,
    pub marker: bool,
    pub payload: Bytes,
}

#[derive(Debug)]
pub struct AccessUnit {
    pub annex_b: Vec<u8>,
    pub rtp_timestamp: u32,
    /// Whether the access unit is an IRAP that can restart decoding.
    #[allow(dead_code)]
    pub keyframe: bool,
}

#[derive(Debug)]
pub enum AssemblerOutput {
    Frame(AccessUnit),
    /// Video was lost and the decoder needs a keyframe. Reported once per
    /// outage, not once per discarded access unit.
    Loss,
}

pub struct RtpAssembler {
    packets: BTreeMap<u64, RtpVideoPacket>,
    next: Option<u64>,
    highest: Option<u64>,
    gap_since: Option<Instant>,
    loss_timeout: Duration,
    awaiting_keyframe: bool,
    ready: VecDeque<AssemblerOutput>,
}

impl Default for RtpAssembler {
    fn default() -> Self {
        Self {
            packets: BTreeMap::new(),
            next: None,
            highest: None,
            gap_since: None,
            loss_timeout: DEFAULT_LOSS_TIMEOUT,
            awaiting_keyframe: true,
            ready: VecDeque::new(),
        }
    }
}

impl RtpAssembler {
    #[allow(dead_code)]
    pub fn set_loss_timeout(&mut self, timeout: Duration) {
        self.loss_timeout = timeout;
    }

    #[allow(dead_code)]
    pub fn awaiting_keyframe(&self) -> bool {
        self.awaiting_keyframe
    }

    pub fn push(&mut self, packet: RtpVideoPacket, now: Instant) {
        let extended = self.extend(packet.sequence);
        if self.next.is_some_and(|next| extended < next) || self.packets.contains_key(&extended) {
            return;
        }
        self.highest = Some(
            self.highest
                .map_or(extended, |highest| highest.max(extended)),
        );
        let next = *self.next.get_or_insert(extended);
        self.packets.insert(extended, packet);
        if extended == next {
            self.gap_since = None;
        }
        self.drain(now);
    }

    pub fn poll(&mut self, now: Instant) -> Option<AssemblerOutput> {
        if self.ready.is_empty() {
            self.drain(now);
        }
        self.ready.pop_front()
    }

    /// Unwraps a 16-bit sequence number against the highest one seen, so
    /// ordering survives wraparound. The first packet starts high enough that
    /// reordering below it cannot go negative.
    fn extend(&self, sequence: u16) -> u64 {
        let Some(highest) = self.highest else {
            return (1u64 << 32) + u64::from(sequence);
        };
        let delta = i64::from(sequence.wrapping_sub(highest as u16) as i16);
        (highest as i64 + delta).max(0) as u64
    }

    fn drain(&mut self, now: Instant) {
        loop {
            let Some(next) = self.next else {
                return;
            };
            match self.complete_unit(next) {
                Some(end) => {
                    let unit: Vec<RtpVideoPacket> = (next..=end)
                        .filter_map(|sequence| self.packets.remove(&sequence))
                        .collect();
                    self.next = Some(end + 1);
                    self.gap_since = None;
                    self.emit(&unit);
                }
                None => {
                    if self.packets.is_empty() {
                        return;
                    }
                    let gap_started = *self.gap_since.get_or_insert(now);
                    let expired = now.saturating_duration_since(gap_started) >= self.loss_timeout;
                    if !expired && self.packets.len() <= MAX_BUFFERED_PACKETS {
                        return;
                    }
                    self.skip_damaged(next);
                }
            }
        }
    }

    /// Returns the last sequence number of the access unit starting at `next`
    /// when every packet of it is present.
    fn complete_unit(&self, next: u64) -> Option<u64> {
        let first = self.packets.get(&next)?;
        let mut sequence = next;
        let mut packet = first;
        loop {
            if packet.marker {
                return Some(sequence);
            }
            let following = self.packets.get(&(sequence + 1))?;
            if following.timestamp != first.timestamp {
                return Some(sequence);
            }
            sequence += 1;
            packet = following;
        }
    }

    /// Discards the damaged access unit and resumes at the packet after the
    /// next marker; with no marker buffered, everything is dropped and
    /// reassembly resumes with whatever arrives next.
    fn skip_damaged(&mut self, next: u64) {
        self.signal_loss();
        let resume = self
            .packets
            .range(next..)
            .find(|(_, packet)| packet.marker)
            .map(|(sequence, _)| sequence + 1);
        match resume {
            Some(resume) => {
                self.packets = self.packets.split_off(&resume);
                self.next = Some(resume);
            }
            None => {
                self.packets.clear();
                self.next = self.highest.map(|highest| highest + 1);
            }
        }
        self.gap_since = None;
    }

    fn emit(&mut self, unit: &[RtpVideoPacket]) {
        let Some(first) = unit.first() else {
            return;
        };
        match depacketize(unit) {
            Some((annex_b, keyframe)) => {
                if self.awaiting_keyframe && !keyframe {
                    return;
                }
                self.awaiting_keyframe = false;
                self.ready.push_back(AssemblerOutput::Frame(AccessUnit {
                    annex_b,
                    rtp_timestamp: first.timestamp,
                    keyframe,
                }));
            }
            None => self.signal_loss(),
        }
    }

    fn signal_loss(&mut self) {
        if !self.awaiting_keyframe {
            self.awaiting_keyframe = true;
            self.ready.push_back(AssemblerOutput::Loss);
        }
    }
}

fn nal_type(header: u8) -> u8 {
    (header >> 1) & 0x3F
}

fn is_irap(nal: &[u8]) -> bool {
    nal.first()
        .is_some_and(|header| (16..=21).contains(&nal_type(*header)))
}

/// Converts one access unit's RTP payloads into Annex-B. Returns `None` for
/// any structural damage: a truncated aggregation, an unterminated or
/// orphaned fragment, or a payload type this stream never negotiates.
fn depacketize(unit: &[RtpVideoPacket]) -> Option<(Vec<u8>, bool)> {
    let mut annex_b = Vec::with_capacity(unit.iter().map(|packet| packet.payload.len() + 8).sum());
    let mut keyframe = false;
    let mut fragment: Option<Vec<u8>> = None;
    let mut append = |annex_b: &mut Vec<u8>, nal: &[u8]| {
        keyframe |= is_irap(nal);
        annex_b.extend_from_slice(&START_CODE);
        annex_b.extend_from_slice(nal);
    };

    for packet in unit {
        let payload = &packet.payload[..];
        if payload.len() < 2 {
            return None;
        }
        match nal_type(payload[0]) {
            NAL_AGGREGATION => {
                if fragment.is_some() {
                    return None;
                }
                let mut offset = 2;
                while offset < payload.len() {
                    let size_bytes = payload.get(offset..offset + 2)?;
                    let size = usize::from(u16::from_be_bytes([size_bytes[0], size_bytes[1]]));
                    offset += 2;
                    let nal = payload.get(offset..offset + size)?;
                    if size < 2 {
                        return None;
                    }
                    append(&mut annex_b, nal);
                    offset += size;
                }
            }
            NAL_FRAGMENT => {
                let header = *payload.get(2)?;
                let start = header & 0x80 != 0;
                let end = header & 0x40 != 0;
                let body = &payload[3..];
                if start {
                    if end || fragment.is_some() {
                        return None;
                    }
                    let mut nal = Vec::with_capacity(body.len() * 8 + 2);
                    nal.push((payload[0] & 0x81) | ((header & 0x3F) << 1));
                    nal.push(payload[1]);
                    nal.extend_from_slice(body);
                    fragment = Some(nal);
                } else {
                    fragment.as_mut()?.extend_from_slice(body);
                }
                if end {
                    let nal = fragment.take()?;
                    append(&mut annex_b, &nal);
                }
            }
            NAL_PACI => return None,
            _ => {
                if fragment.is_some() {
                    return None;
                }
                append(&mut annex_b, payload);
            }
        }
    }

    if fragment.is_some() || annex_b.is_empty() {
        return None;
    }
    Some((annex_b, keyframe))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDR: u8 = 19;
    const TRAIL: u8 = 1;
    const VPS: u8 = 32;

    fn nal(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut nal = vec![kind << 1, 1];
        nal.extend_from_slice(body);
        nal
    }

    fn packet(sequence: u16, timestamp: u32, marker: bool, payload: Vec<u8>) -> RtpVideoPacket {
        RtpVideoPacket {
            sequence,
            timestamp,
            marker,
            payload: Bytes::from(payload),
        }
    }

    fn fragments(kind: u8, body: &[u8], pieces: usize) -> Vec<Vec<u8>> {
        let chunk = body.len().div_ceil(pieces);
        body.chunks(chunk)
            .enumerate()
            .map(|(index, piece)| {
                let mut header = kind;
                if index == 0 {
                    header |= 0x80;
                }
                if index == pieces - 1 {
                    header |= 0x40;
                }
                let mut payload = vec![NAL_FRAGMENT << 1, 1, header];
                payload.extend_from_slice(piece);
                payload
            })
            .collect()
    }

    fn aggregate(nals: &[Vec<u8>]) -> Vec<u8> {
        let mut payload = vec![NAL_AGGREGATION << 1, 1];
        for nal in nals {
            payload.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            payload.extend_from_slice(nal);
        }
        payload
    }

    fn annex_b(nals: &[Vec<u8>]) -> Vec<u8> {
        nals.iter()
            .flat_map(|nal| START_CODE.iter().copied().chain(nal.iter().copied()))
            .collect()
    }

    fn keyframe_unit(first: u16, timestamp: u32) -> (Vec<RtpVideoPacket>, Vec<u8>) {
        let vps = nal(VPS, &[7, 7]);
        let idr_body: Vec<u8> = (0..900).map(|index| (index % 200) as u8 + 1).collect();
        let idr = nal(IDR, &idr_body);
        let mut packets = vec![packet(first, timestamp, false, aggregate(&[vps.clone()]))];
        let pieces = fragments(IDR, &idr_body, 3);
        let count = pieces.len();
        for (index, piece) in pieces.into_iter().enumerate() {
            packets.push(packet(
                first.wrapping_add(1 + index as u16),
                timestamp,
                index == count - 1,
                piece,
            ));
        }
        (packets, annex_b(&[vps, idr]))
    }

    fn frames(assembler: &mut RtpAssembler, now: Instant) -> Vec<AssemblerOutput> {
        std::iter::from_fn(|| assembler.poll(now)).collect()
    }

    #[test]
    fn reassembles_aggregated_and_fragmented_nals_out_of_order() {
        let now = Instant::now();
        let (mut packets, expected) = keyframe_unit(100, 9000);
        packets.reverse();
        let mut assembler = RtpAssembler::default();
        let first = packets.pop().expect("first packet");
        assembler.push(first, now);
        for packet in packets {
            assembler.push(packet, now);
        }
        match frames(&mut assembler, now).as_slice() {
            [AssemblerOutput::Frame(unit)] => {
                assert!(unit.keyframe);
                assert_eq!(unit.rtp_timestamp, 9000);
                assert_eq!(unit.annex_b, expected);
            }
            other => panic!("unexpected output: {other:?}"),
        }
    }

    #[test]
    fn follows_sequence_wraparound_and_ignores_duplicates() {
        let now = Instant::now();
        let mut assembler = RtpAssembler::default();
        let (packets, _) = keyframe_unit(u16::MAX - 1, 1);
        for packet in &packets {
            assembler.push(packet.clone(), now);
            assembler.push(packet.clone(), now);
        }
        let trail = nal(TRAIL, &[9, 9, 9]);
        assembler.push(
            packet(packets.len() as u16 - 2, 2, true, trail.clone()),
            now,
        );
        let outputs = frames(&mut assembler, now);
        assert_eq!(outputs.len(), 2, "{outputs:?}");
        match &outputs[1] {
            AssemblerOutput::Frame(unit) => {
                assert!(!unit.keyframe);
                assert_eq!(unit.annex_b, annex_b(&[trail]));
            }
            other => panic!("unexpected output: {other:?}"),
        }
    }

    #[test]
    fn waits_for_a_late_packet_inside_the_loss_timeout() {
        let start = Instant::now();
        let mut assembler = RtpAssembler::default();
        let (packets, expected) = keyframe_unit(10, 1);
        for (index, packet) in packets.iter().enumerate() {
            if index != 2 {
                assembler.push(packet.clone(), start);
            }
        }
        assert!(assembler.poll(start + Duration::from_millis(40)).is_none());
        assembler.push(packets[2].clone(), start + Duration::from_millis(50));
        match frames(&mut assembler, start + Duration::from_millis(50)).as_slice() {
            [AssemblerOutput::Frame(unit)] => assert_eq!(unit.annex_b, expected),
            other => panic!("unexpected output: {other:?}"),
        }
    }

    #[test]
    fn declares_loss_and_gates_on_the_next_keyframe() {
        let start = Instant::now();
        let mut assembler = RtpAssembler::default();
        let (packets, _) = keyframe_unit(10, 1);
        for packet in &packets {
            assembler.push(packet.clone(), start);
        }
        assert!(matches!(
            frames(&mut assembler, start).as_slice(),
            [AssemblerOutput::Frame(_)]
        ));

        // A delta frame loses its middle fragment.
        let delta_body: Vec<u8> = (0..300).map(|index| (index % 100) as u8 + 1).collect();
        let delta = fragments(TRAIL, &delta_body, 3);
        assembler.push(packet(14, 2, false, delta[0].clone()), start);
        assembler.push(packet(16, 2, true, delta[2].clone()), start);
        // The next delta frame is intact but references the lost one.
        assembler.push(packet(17, 3, true, nal(TRAIL, &[1, 2])), start);
        assert!(assembler.poll(start + Duration::from_millis(10)).is_none());

        let later = start + DEFAULT_LOSS_TIMEOUT + Duration::from_millis(1);
        assert!(matches!(
            frames(&mut assembler, later).as_slice(),
            [AssemblerOutput::Loss]
        ));
        assert!(assembler.awaiting_keyframe());

        let (recovery, expected) = keyframe_unit(18, 4);
        for packet in recovery {
            assembler.push(packet, later);
        }
        match frames(&mut assembler, later).as_slice() {
            [AssemblerOutput::Frame(unit)] => {
                assert!(unit.keyframe);
                assert_eq!(unit.annex_b, expected);
            }
            other => panic!("unexpected output: {other:?}"),
        }
    }

    #[test]
    fn drops_delta_frames_before_the_first_keyframe() {
        let now = Instant::now();
        let mut assembler = RtpAssembler::default();
        assembler.push(packet(1, 1, true, nal(TRAIL, &[1])), now);
        assert!(frames(&mut assembler, now).is_empty());
        let (packets, _) = keyframe_unit(2, 2);
        for packet in packets {
            assembler.push(packet, now);
        }
        assert!(matches!(
            frames(&mut assembler, now).as_slice(),
            [AssemblerOutput::Frame(unit)] if unit.keyframe
        ));
    }

    #[test]
    fn rejects_structurally_damaged_units() {
        let orphan = fragments(TRAIL, &[1, 2, 3, 4, 5, 6], 3);
        assert!(depacketize(&[packet(1, 1, true, orphan[1].clone())]).is_none());
        assert!(depacketize(&[packet(1, 1, true, orphan[0].clone())]).is_none());
        let mut truncated = aggregate(&[nal(TRAIL, &[1, 2, 3])]);
        truncated.pop();
        assert!(depacketize(&[packet(1, 1, true, truncated)]).is_none());
        assert!(depacketize(&[packet(1, 1, true, vec![NAL_PACI << 1, 1, 0, 0])]).is_none());
    }
}
