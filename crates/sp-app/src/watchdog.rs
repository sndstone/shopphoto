//! Main-thread stall watchdog.
//!
//! macOS shows the beach ball when the main thread stops servicing events
//! for a few seconds. The eframe `logic` pass is the only place ShopPhoto
//! does work on that thread, so timing its sections is enough to name the
//! culprit. A frame whose sections add up to more than the threshold is
//! logged at `warn` with the slowest sections first, and the same summary is
//! kept for the F12 overlay.
//!
//! Set `SHOPPHOTO_STALL_MS` to change the threshold (default 100 ms).

use std::time::{Duration, Instant};

pub struct Watchdog {
    started: Instant,
    last: Instant,
    laps: Vec<(&'static str, Duration)>,
    threshold: Duration,
}

impl Watchdog {
    pub fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last: now,
            laps: Vec::with_capacity(16),
            threshold: threshold_from_env(),
        }
    }

    #[cfg(test)]
    fn with_threshold(threshold: Duration) -> Self {
        let mut watchdog = Self::start();
        watchdog.threshold = threshold;
        watchdog
    }

    /// Close the section that ran since the previous lap (or since start).
    pub fn lap(&mut self, name: &'static str) {
        let now = Instant::now();
        self.laps.push((name, now.duration_since(self.last)));
        self.last = now;
    }

    /// Log the frame if it exceeded the threshold and return the summary.
    pub fn finish(self) -> Option<String> {
        let total = self.started.elapsed();
        if total < self.threshold {
            return None;
        }
        let summary = summarize(total, &self.laps);
        tracing::warn!(
            total_ms = total.as_millis() as u64,
            "main-thread stall: {summary}"
        );
        Some(summary)
    }
}

fn threshold_from_env() -> Duration {
    std::env::var("SHOPPHOTO_STALL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_millis(100), Duration::from_millis)
}

/// `"312 ms: poll_background 280, sidecar 21, rerender 9"` — sections that
/// contributed under one percent are left out so the culprit is obvious.
fn summarize(total: Duration, laps: &[(&'static str, Duration)]) -> String {
    let mut sorted: Vec<_> = laps.to_vec();
    sorted.sort_by_key(|(_, elapsed)| std::cmp::Reverse(*elapsed));
    let floor = total / 100;
    let parts: Vec<String> = sorted
        .iter()
        .filter(|(_, elapsed)| *elapsed >= floor)
        .take(5)
        .map(|(name, elapsed)| format!("{name} {}", elapsed.as_millis()))
        .collect();
    format!("{} ms: {}", total.as_millis(), parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_frames_produce_no_report() {
        let mut watchdog = Watchdog::with_threshold(Duration::from_secs(10));
        watchdog.lap("a");
        assert!(watchdog.finish().is_none());
    }

    #[test]
    fn slow_frames_name_the_slowest_section_first() {
        let mut watchdog = Watchdog::with_threshold(Duration::ZERO);
        watchdog.lap("fast");
        std::thread::sleep(Duration::from_millis(12));
        watchdog.lap("slow");
        let report = watchdog.finish().expect("threshold is zero");
        assert!(report.contains("ms: slow "), "{report}");
    }

    #[test]
    fn summary_drops_negligible_sections() {
        let laps = [
            ("big", Duration::from_millis(500)),
            ("tiny", Duration::from_micros(10)),
        ];
        let text = summarize(Duration::from_millis(500), &laps);
        assert_eq!(text, "500 ms: big 500");
    }
}
