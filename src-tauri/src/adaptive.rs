const MIN_BITRATE_MBPS: f64 = 2.0;
const MAX_CEILING_MBPS: f64 = 200.0;
const DEGRADE_FACTOR: f64 = 0.8;
const RAMP_FACTOR: f64 = 1.15;
const HEALTHY_REPORTS_TO_RAMP: u8 = 3;
const MIN_SENT_FRAMES: u32 = 4;
const DEGRADE_DELIVERY_RATIO: f64 = 0.9;
const HEALTHY_DELIVERY_RATIO: f64 = 0.97;
const DEGRADE_JITTER_MS: f64 = 90.0;
const HEALTHY_JITTER_MS: f64 = 40.0;
const REPORT_INTERVAL_MS: u64 = 1_000;

#[derive(Debug)]
pub struct AdaptiveRate {
    enabled: bool,
    ceiling_mbps: f64,
    current_mbps: f64,
    healthy_reports: u8,
}

impl AdaptiveRate {
    pub fn new(enabled: bool, ceiling_mbps: f64) -> Self {
        let ceiling = ceiling_mbps.clamp(MIN_BITRATE_MBPS, MAX_CEILING_MBPS);
        Self {
            enabled,
            ceiling_mbps: ceiling,
            current_mbps: ceiling,
            healthy_reports: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn current_mbps(&self) -> f64 {
        self.current_mbps
    }

    pub fn ceiling_mbps(&self) -> f64 {
        self.ceiling_mbps
    }

    /// One-step cut used when the host's own writes are blocking; the caller
    /// throttles how often it calls this.
    pub fn degrade(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        self.healthy_reports = 0;
        self.set_current((self.current_mbps * DEGRADE_FACTOR).max(MIN_BITRATE_MBPS))
    }

    /// Consumes one client feedback window. Client and host windows are only
    /// approximately aligned, so degrade thresholds are deliberately loose.
    pub fn observe(&mut self, received: u32, sent: u32, jitter_ms: f64, dropped: u32) -> bool {
        if !self.enabled {
            return false;
        }
        let flowing = sent >= MIN_SENT_FRAMES;
        let delivery = if flowing {
            (f64::from(received) / f64::from(sent)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let congested = (flowing && delivery < DEGRADE_DELIVERY_RATIO)
            || dropped > 0
            || jitter_ms > DEGRADE_JITTER_MS;

        if congested {
            return self.degrade();
        }

        let healthy = flowing
            && delivery >= HEALTHY_DELIVERY_RATIO
            && jitter_ms <= HEALTHY_JITTER_MS
            && dropped == 0;
        if !healthy {
            self.healthy_reports = 0;
            return false;
        }

        self.healthy_reports = self.healthy_reports.saturating_add(1);
        if self.healthy_reports < HEALTHY_REPORTS_TO_RAMP || self.current_mbps >= self.ceiling_mbps
        {
            return false;
        }
        self.healthy_reports = 0;
        let stepped = (self.current_mbps * RAMP_FACTOR)
            .max(self.current_mbps + 1.0)
            .min(self.ceiling_mbps);
        self.set_current(stepped)
    }

    fn set_current(&mut self, next: f64) -> bool {
        let next = (next * 10.0).round() / 10.0;
        if (next - self.current_mbps).abs() < 0.05 {
            return false;
        }
        self.current_mbps = next;
        true
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LinkReport {
    pub received: u32,
    pub dropped: u32,
    pub jitter_ms: f64,
}

/// Tracks arrival timing and frame-id continuity on the client. Timestamps are
/// host-side, so jitter compares arrival deltas against frame timestamp deltas
/// rather than absolute clocks.
#[derive(Debug)]
pub struct LinkMonitor {
    window_started_ms: u64,
    received: u32,
    dropped: u32,
    total_dropped: u64,
    jitter_ms: f64,
    last_arrival_ms: Option<u64>,
    last_timestamp_ms: Option<u64>,
    last_frame_id: Option<u32>,
}

impl LinkMonitor {
    pub fn new(started_ms: u64) -> Self {
        Self {
            window_started_ms: started_ms,
            received: 0,
            dropped: 0,
            total_dropped: 0,
            jitter_ms: 0.0,
            last_arrival_ms: None,
            last_timestamp_ms: None,
            last_frame_id: None,
        }
    }

    pub fn observe_frame(&mut self, frame_id: u32, timestamp_ms: u64, arrival_ms: u64) -> bool {
        if let (Some(previous), Some(last_arrival), Some(last_timestamp)) = (
            self.last_frame_id,
            self.last_arrival_ms,
            self.last_timestamp_ms,
        ) {
            if let Some(missing) = frame_id.checked_sub(previous).filter(|gap| *gap > 1) {
                let missing = missing - 1;
                self.dropped = self.dropped.saturating_add(missing);
                self.total_dropped = self.total_dropped.saturating_add(u64::from(missing));
            }
            if frame_id > previous {
                let arrival_delta = arrival_ms.saturating_sub(last_arrival) as f64;
                let timestamp_delta = timestamp_ms.saturating_sub(last_timestamp) as f64;
                let deviation = (arrival_delta - timestamp_delta).abs().min(5_000.0);
                self.jitter_ms = if self.jitter_ms == 0.0 {
                    deviation
                } else {
                    self.jitter_ms * 0.75 + deviation * 0.25
                };
            }
        }

        let gap = self
            .last_frame_id
            .is_some_and(|previous| frame_id > previous.saturating_add(1));
        self.last_frame_id = Some(frame_id);
        self.last_arrival_ms = Some(arrival_ms);
        self.last_timestamp_ms = Some(timestamp_ms);
        self.received = self.received.saturating_add(1);
        gap
    }

    pub fn observe_failure(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
        self.total_dropped = self.total_dropped.saturating_add(1);
    }

    pub fn report(&mut self, now_ms: u64) -> Option<LinkReport> {
        if now_ms.saturating_sub(self.window_started_ms) < REPORT_INTERVAL_MS {
            return None;
        }
        let report = LinkReport {
            received: std::mem::take(&mut self.received),
            dropped: std::mem::take(&mut self.dropped),
            jitter_ms: self.jitter_ms,
        };
        self.window_started_ms = now_ms;
        Some(report)
    }

    pub fn jitter_ms(&self) -> f64 {
        self.jitter_ms
    }

    pub fn dropped_frames(&self) -> u64 {
        self.total_dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrades_when_delivery_falls_behind() {
        let mut rate = AdaptiveRate::new(true, 40.0);
        assert!(rate.observe(20, 40, 0.0, 0));
        assert!((rate.current_mbps() - 32.0).abs() < 0.01);
        assert!(rate.observe(20, 40, 0.0, 0));
        assert!((rate.current_mbps() - 25.6).abs() < 0.01);
    }

    #[test]
    fn degrades_on_jitter_or_dropped_frames() {
        let mut rate = AdaptiveRate::new(true, 20.0);
        assert!(rate.observe(30, 30, 120.0, 0));
        assert_eq!(rate.current_mbps(), 16.0);

        let mut rate = AdaptiveRate::new(true, 20.0);
        assert!(rate.observe(30, 30, 10.0, 2));
        assert_eq!(rate.current_mbps(), 16.0);
    }

    #[test]
    fn ramps_up_only_after_healthy_windows() {
        let mut rate = AdaptiveRate::new(true, 20.0);
        assert!(rate.observe(10, 20, 0.0, 0));
        assert_eq!(rate.current_mbps(), 16.0);

        assert!(!rate.observe(20, 20, 5.0, 0));
        assert!(!rate.observe(20, 20, 5.0, 0));
        assert_eq!(rate.current_mbps(), 16.0);
        assert!(rate.observe(20, 20, 5.0, 0));
        assert!(rate.current_mbps() > 16.0);

        for _ in 0..50 {
            rate.observe(20, 20, 5.0, 0);
        }
        assert_eq!(rate.current_mbps(), 20.0);
    }

    #[test]
    fn never_goes_below_the_floor_or_moves_when_disabled() {
        let mut rate = AdaptiveRate::new(true, 10.0);
        for _ in 0..40 {
            rate.observe(0, 30, 200.0, 5);
        }
        assert_eq!(rate.current_mbps(), MIN_BITRATE_MBPS);

        let mut fixed = AdaptiveRate::new(false, 24.0);
        assert!(!fixed.observe(0, 30, 200.0, 3));
        assert_eq!(fixed.current_mbps(), 24.0);
        assert!(!fixed.enabled());
    }

    #[test]
    fn ignores_idle_windows_for_delivery_but_keeps_the_rate() {
        let mut rate = AdaptiveRate::new(true, 30.0);
        assert!(!rate.observe(0, 0, 0.0, 0));
        assert_eq!(rate.current_mbps(), 30.0);
    }

    #[test]
    fn monitor_reports_delivery_jitter_and_gaps() {
        let mut monitor = LinkMonitor::new(0);
        for index in 0..30u32 {
            let arrival = u64::from(index) * 33;
            monitor.observe_frame(index, arrival, arrival + 4);
        }
        let report = monitor.report(1_000).expect("one-second report");
        assert_eq!(report.received, 30);
        assert_eq!(report.dropped, 0);
        assert!(report.jitter_ms < 1.0);
        assert!(monitor.report(1_500).is_none());
    }

    #[test]
    fn monitor_detects_missing_frames_and_arrival_delay() {
        let mut monitor = LinkMonitor::new(0);
        monitor.observe_frame(1, 0, 0);
        monitor.observe_frame(2, 33, 33);
        assert!(monitor.observe_frame(6, 66, 120));
        assert_eq!(monitor.dropped_frames(), 3);

        monitor.observe_failure();
        assert_eq!(monitor.dropped_frames(), 4);
        let report = monitor.report(1_000).expect("report");
        assert_eq!(report.received, 3);
        assert_eq!(report.dropped, 4);
        assert!(report.jitter_ms > 10.0);
    }
}
