//! Delay-based send-rate estimation for the RTP video track.
//!
//! The delay signal comes from rtc-interceptor's GCC building blocks (arrival
//! grouping, Kalman slope filter and overuse detector fed by TWCC feedback).
//! The rate policy on top of them is ours: a screen encoder is idle most of
//! the time and bursts on change, so a sender-side "received × 0.85" backoff
//! taken while the link is idle would collapse the target for no reason.
//!
//! The slope detector alone only reacts once the send rate is several times
//! the link rate, so a standing-queue check (one-way delay above its windowed
//! minimum, the base-delay idea from RFC 6817) also counts as overuse.

use rtc::interceptor::{
    BandwidthEstimator, EstimatorStats, LossController, OveruseDetector, PacketReport,
    RateControlState, SlopeEstimator, Usage,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// The pacer drains at this multiple of the link estimate so a keyframe burst
/// is spread over a few frame intervals instead of a single wire-rate spike.
pub const PACING_FACTOR: f64 = 1.2;
/// Wire bits per media bit with the FlexFEC block of 10 media + 2 repair.
pub const FEC_OVERHEAD: f64 = 1.2;
/// Share of the link the encoder may target; the rest absorbs RTP/SRTP headers.
const ENCODER_SHARE: f64 = 0.9;

const CHANGE_INTERVAL: Duration = Duration::from_millis(200);
const DECREASE_FACTOR: f64 = 0.85;
const FAST_GROWTH_PER_SECOND: f64 = 1.25;
const CAREFUL_GROWTH_PER_SECOND: f64 = 1.06;
const BACKOFF_MEMORY: Duration = Duration::from_secs(10);
/// Below this share of the target the sender was idle, not link-limited.
const LINK_LIMITED_SHARE: f64 = 0.6;
/// Queuing delay that every packet of a feedback batch carries before the
/// path is considered congested.
const STANDING_QUEUE_LIMIT_MS: f64 = 15.0;

/// A jump this large means the receiver's 24-bit TWCC reference wrapped.
const BASE_DELAY_RESET_MS: f64 = 60_000.0;
const BASE_DELAY_RESET_WINDOW: Duration = Duration::from_secs(60);

const DELIVERY_WINDOW: Duration = Duration::from_millis(500);

/// Receive rate from the receiver's own arrival clock, so feedback cadence and
/// RTT jitter on the return path do not distort it.
#[derive(Default)]
struct DeliveryRate {
    samples: VecDeque<(Duration, usize)>,
}

impl DeliveryRate {
    fn add(&mut self, arrival: Duration, size: usize) {
        if self
            .samples
            .back()
            .is_some_and(|(newest, _)| arrival + BASE_DELAY_RESET_WINDOW < *newest)
        {
            self.samples.clear();
        }
        self.samples.push_back((arrival, size));
        while let (Some((oldest, _)), Some((newest, _))) =
            (self.samples.front(), self.samples.back())
        {
            if newest.saturating_sub(*oldest) <= DELIVERY_WINDOW {
                break;
            }
            self.samples.pop_front();
        }
    }

    fn bits_per_second(&self) -> Option<f64> {
        let (oldest, _) = self.samples.front()?;
        let (newest, _) = self.samples.back()?;
        let span = newest.checked_sub(*oldest)?;
        if span < Duration::from_millis(20) {
            return None;
        }
        let bytes: usize = self.samples.iter().skip(1).map(|(_, size)| size).sum();
        Some(bytes as f64 * 8.0 / span.as_secs_f64())
    }
}

/// The estimate as seen by the session thread, published without locks.
#[derive(Debug)]
pub struct LinkEstimate {
    target_bps: AtomicU64,
    loss: AtomicU64,
}

impl LinkEstimate {
    pub fn new(initial_bps: f64) -> Self {
        Self {
            target_bps: AtomicU64::new(initial_bps.to_bits()),
            loss: AtomicU64::new(0f64.to_bits()),
        }
    }

    /// Wire bitrate the link is believed to carry, FEC included.
    pub fn target_bps(&self) -> f64 {
        f64::from_bits(self.target_bps.load(Ordering::Relaxed))
    }

    /// Fraction of reported packets lost. Kept for session diagnostics.
    #[allow(dead_code)]
    pub fn loss(&self) -> f64 {
        f64::from_bits(self.loss.load(Ordering::Relaxed))
    }

    fn publish(&self, target_bps: f64, loss: f64) {
        self.target_bps
            .store(target_bps.to_bits(), Ordering::Relaxed);
        self.loss.store(loss.to_bits(), Ordering::Relaxed);
    }
}

/// Encoder bitrate in Mbps that fits a wire estimate, bounded to `2..=ceiling`.
pub fn encoder_mbps_for(wire_bps: f64, ceiling_mbps: f64) -> f64 {
    (wire_bps / FEC_OVERHEAD * ENCODER_SHARE / 1_000_000.0).clamp(2.0, ceiling_mbps.max(2.0))
}

/// Wire bitrate needed to carry an encoder running at `mbps`.
pub fn wire_bps_for(mbps: f64) -> f64 {
    mbps * 1_000_000.0 * FEC_OVERHEAD / ENCODER_SHARE
}

pub struct ScreenEstimator {
    slope: SlopeEstimator,
    detector: OveruseDetector,
    received: DeliveryRate,
    loss: LossController,
    state: RateControlState,
    delay_target: f64,
    min: f64,
    max: f64,
    last_change: Option<Instant>,
    last_backoff: Option<(Instant, f64)>,
    target: f64,
    shared: Arc<LinkEstimate>,
    epoch: Option<Instant>,
    /// Session-minimum one-way delay. Kept for the whole session (not a rolling
    /// window) so a standing queue cannot be mistaken for the new normal.
    base_delay_floor: Option<f64>,
}

impl ScreenEstimator {
    pub fn new(initial: f64, min: f64, max: f64, shared: Arc<LinkEstimate>) -> Self {
        let initial = initial.clamp(min, max);
        shared.publish(initial, 0.0);
        Self {
            slope: SlopeEstimator::new(),
            detector: OveruseDetector::new(),
            received: DeliveryRate::default(),
            loss: LossController::new(initial, min, max),
            state: RateControlState::default(),
            delay_target: initial,
            min,
            max,
            last_change: None,
            last_backoff: None,
            target: initial,
            shared,
            epoch: None,
            base_delay_floor: None,
        }
    }

    /// Smallest queuing delay any packet in the batch saw, in ms. The batch
    /// minimum ignores per-packet jitter and burst transients, and the floor is
    /// the smallest value seen for the whole session (RFC 6817 base delay), so a
    /// queue that builds slowly over minutes is still measured against the path
    /// when it was empty.
    fn standing_queue_ms(&mut self, reports: &[PacketReport]) -> Option<f64> {
        let epoch = *self.epoch.get_or_insert_with(|| {
            reports
                .iter()
                .map(|report| report.departure)
                .min()
                .unwrap_or_else(Instant::now)
        });
        let batch_min = reports
            .iter()
            .filter_map(|report| {
                let arrival = report.arrival?;
                let departure = report.departure.checked_duration_since(epoch)?;
                Some((arrival.as_secs_f64() - departure.as_secs_f64()) * 1000.0)
            })
            .reduce(f64::min)?;

        if self
            .base_delay_floor
            .is_some_and(|base| (batch_min - base).abs() > BASE_DELAY_RESET_MS)
        {
            self.base_delay_floor = None;
        }
        let floor = match self.base_delay_floor {
            Some(base) => base.min(batch_min),
            None => batch_min,
        };
        self.base_delay_floor = Some(floor);
        Some(batch_min - floor)
    }

    fn may_change(&self, now: Instant) -> bool {
        self.last_change
            .is_none_or(|last| now.saturating_duration_since(last) >= CHANGE_INTERVAL)
    }

    fn decrease(&mut self, now: Instant, received: Option<f64>) {
        let current = self.delay_target;
        let floor = current * 0.5;
        let backed_off = match received {
            Some(rate) => (rate * DECREASE_FACTOR).max(floor),
            None => current * DECREASE_FACTOR,
        };
        self.delay_target = current.min(backed_off).clamp(self.min, self.max);
        if received.is_some_and(|rate| rate >= current * LINK_LIMITED_SHARE) {
            self.last_backoff = Some((now, current));
        }
        self.last_change = Some(now);
    }

    fn increase(&mut self, now: Instant) {
        let elapsed = self
            .last_change
            .map(|last| now.saturating_duration_since(last))
            .unwrap_or(CHANGE_INTERVAL)
            .min(Duration::from_secs(1));
        if self
            .last_backoff
            .is_some_and(|(at, _)| now.saturating_duration_since(at) > BACKOFF_MEMORY)
        {
            self.last_backoff = None;
        }
        let near_ceiling = self
            .last_backoff
            .is_some_and(|(_, rate)| self.delay_target > rate * 0.9);
        let growth = if near_ceiling {
            CAREFUL_GROWTH_PER_SECOND
        } else {
            FAST_GROWTH_PER_SECOND
        };
        self.delay_target =
            (self.delay_target * growth.powf(elapsed.as_secs_f64())).clamp(self.min, self.max);
        self.last_change = Some(now);
    }
}

impl BandwidthEstimator for ScreenEstimator {
    fn on_reports(&mut self, now: Instant, reports: &[PacketReport]) {
        if reports.is_empty() {
            return;
        }

        let mut usage = self.detector.usage();
        for report in reports {
            if let Some(arrival) = report.arrival.filter(|_| report.arrived) {
                self.received.add(arrival, report.size);
            }
            if let Some(trend) = self.slope.accumulate(report) {
                usage = self.detector.update(trend.at, trend.estimate_ms);
            }
        }

        if self
            .standing_queue_ms(reports)
            .is_some_and(|queue| queue > STANDING_QUEUE_LIMIT_MS)
        {
            usage = Usage::Over;
        }

        self.state = self.state.next(usage);
        if self.may_change(now) {
            match self.state {
                RateControlState::Decrease => {
                    let received = self.received.bits_per_second();
                    self.decrease(now, received);
                }
                RateControlState::Increase => self.increase(now),
                RateControlState::Hold => {}
            }
        }

        let lost = reports.iter().filter(|report| !report.arrived).count();
        let loss_target = self.loss.update(now, lost, reports.len());
        self.target = self.delay_target.min(loss_target).clamp(self.min, self.max);
        self.shared
            .publish(self.target, self.loss.average_loss().unwrap_or(0.0));
    }

    /// The pacer's drain rate. The encoder is sized from the shared estimate
    /// instead, so the pacer only ever holds a fraction of a frame.
    fn target_bitrate(&self) -> f64 {
        self.target * PACING_FACTOR
    }

    fn stats(&self) -> EstimatorStats {
        let mut stats = EstimatorStats::default();
        stats.delay_based_bitrate = Some(self.delay_target);
        stats.loss_based_bitrate = Some(self.loss.target());
        stats.packet_loss = self.loss.average_loss();
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtc::rtcp::transport_feedbacks::cc_feedback_report::Ecn;

    const MBPS: f64 = 1_000_000.0;

    fn estimator(initial: f64) -> (ScreenEstimator, Arc<LinkEstimate>) {
        let shared = Arc::new(LinkEstimate::new(initial));
        (
            ScreenEstimator::new(initial, MBPS, 100.0 * MBPS, shared.clone()),
            shared,
        )
    }

    /// Feeds `batches` of 200 ms feedback carrying `rate_bps` of 1200-byte
    /// packets whose one-way delay grows by `queue_growth_ms` per packet.
    fn feed(
        estimator: &mut ScreenEstimator,
        epoch: Instant,
        start_batch: u64,
        batches: u64,
        rate_bps: f64,
        queue_growth_ms: f64,
    ) -> Instant {
        let size = 1200usize;
        let per_batch = ((rate_bps * 0.2) / (size as f64 * 8.0)).max(1.0) as u64;
        let spacing_us = 200_000 / per_batch;
        let mut id = start_batch * 10_000;
        let mut delay_ms = 20.0;
        let mut now = epoch;
        for batch in start_batch..start_batch + batches {
            let batch_start = epoch + Duration::from_millis(batch * 200);
            let reports: Vec<PacketReport> = (0..per_batch)
                .map(|index| {
                    id += 1;
                    delay_ms += queue_growth_ms;
                    let departure = batch_start + Duration::from_micros(index * spacing_us);
                    let sent_us = (batch * 200_000 + index * spacing_us) as f64;
                    PacketReport {
                        ssrc: 1,
                        id,
                        rtp_sequence_number: id as u16,
                        is_twcc: true,
                        twcc_sequence_number: id as u16,
                        size,
                        arrived: true,
                        departure,
                        arrival: Some(Duration::from_micros((sent_us + delay_ms * 1000.0) as u64)),
                        ecn: Ecn::default(),
                    }
                })
                .collect();
            now = batch_start + Duration::from_millis(200);
            estimator.on_reports(now, &reports);
        }
        now
    }

    #[test]
    fn backs_off_when_the_queue_grows() {
        let epoch = Instant::now();
        let (mut estimator, shared) = estimator(20.0 * MBPS);
        feed(&mut estimator, epoch, 0, 20, 18.0 * MBPS, 0.5);
        assert!(
            shared.target_bps() < 20.0 * MBPS,
            "a growing queue must lower the estimate, got {}",
            shared.target_bps()
        );
        assert_eq!(
            estimator.target_bitrate(),
            shared.target_bps() * PACING_FACTOR
        );
    }

    #[test]
    fn an_idle_overuse_never_halves_the_target_in_one_step() {
        let epoch = Instant::now();
        let (mut estimator, _) = estimator(20.0 * MBPS);
        estimator.decrease(epoch, Some(0.5 * MBPS));
        assert_eq!(estimator.delay_target, 10.0 * MBPS);
        assert!(
            estimator.last_backoff.is_none(),
            "an app-limited overuse must not cap later growth"
        );
    }

    #[test]
    fn climbs_on_a_quiet_path_and_slows_near_the_last_backoff() {
        let epoch = Instant::now();
        let (mut estimator, shared) = estimator(8.0 * MBPS);
        feed(&mut estimator, epoch, 0, 25, 6.0 * MBPS, 0.0);
        let climbed = shared.target_bps();
        assert!(
            climbed > 8.0 * MBPS,
            "a quiet path must climb, got {climbed}"
        );

        let mut careful = ScreenEstimator::new(
            9.5 * MBPS,
            MBPS,
            100.0 * MBPS,
            Arc::new(LinkEstimate::new(9.5 * MBPS)),
        );
        careful.last_backoff = Some((epoch, 10.0 * MBPS));
        careful.last_change = Some(epoch);
        careful.increase(epoch + Duration::from_secs(1));
        assert!((careful.delay_target - 9.5 * MBPS * CAREFUL_GROWTH_PER_SECOND).abs() < 1.0);
    }

    #[test]
    fn stays_within_bounds() {
        let epoch = Instant::now();
        let (mut estimator, shared) = estimator(1.2 * MBPS);
        for step in 0..10u64 {
            estimator.decrease(epoch + Duration::from_secs(step), Some(0.1 * MBPS));
        }
        assert_eq!(estimator.delay_target, MBPS);
        estimator.on_reports(epoch, &[]);
        assert!(shared.target_bps() >= MBPS);
    }

    #[test]
    fn base_delay_is_kept_for_the_whole_session() {
        let epoch = Instant::now();
        let mut estimator = ScreenEstimator::new(
            20.0 * MBPS,
            MBPS,
            100.0 * MBPS,
            Arc::new(LinkEstimate::new(20.0 * MBPS)),
        );
        let report = |departure: Duration, arrival: Duration| PacketReport {
            ssrc: 1,
            id: 1,
            rtp_sequence_number: 1,
            is_twcc: true,
            twcc_sequence_number: 1,
            size: 1200,
            arrived: true,
            departure: epoch + departure,
            arrival: Some(arrival),
            ecn: Ecn::default(),
        };
        // Empty-path baseline, then a queue that appears a minute later.
        assert_eq!(
            estimator.standing_queue_ms(&[report(Duration::ZERO, Duration::from_millis(20))]),
            Some(0.0)
        );
        let queued = estimator.standing_queue_ms(&[report(
            Duration::from_secs(60),
            Duration::from_secs(60) + Duration::from_millis(80),
        )]);
        assert!(
            queued.is_some_and(|queue| queue > STANDING_QUEUE_LIMIT_MS),
            "a queue built after the 30s window must still be measured, got {queued:?}"
        );
    }

    #[test]
    fn sizes_the_encoder_inside_the_link() {
        assert_eq!(encoder_mbps_for(0.0, 50.0), 2.0);
        assert_eq!(encoder_mbps_for(1e12, 50.0), 50.0);
        let wire = wire_bps_for(20.0);
        assert!((encoder_mbps_for(wire, 50.0) - 20.0).abs() < 1e-9);
    }
}
