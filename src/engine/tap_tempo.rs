use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const MIN_BPM: f64 = 40.0;
pub const MAX_BPM: f64 = 300.0;

/// Taps closer together than this are treated as switch bounce / double triggers.
const DEBOUNCE: Duration = Duration::from_millis(100);
/// A gap longer than one beat at MIN_BPM starts a new tap sequence.
const RESET_GAP: Duration = Duration::from_millis(1500);
const MAX_TAPS: usize = 5;

#[derive(Debug)]
pub struct TapTempo {
    taps: VecDeque<Instant>,
    current_bpm: f64,
}

impl TapTempo {
    pub fn new(fallback_bpm: f64) -> Self {
        let bpm = if fallback_bpm.is_finite() {
            fallback_bpm.clamp(MIN_BPM, MAX_BPM)
        } else {
            128.0
        };
        Self {
            taps: VecDeque::with_capacity(MAX_TAPS),
            current_bpm: bpm,
        }
    }

    /// Records a tap. Returns false if the tap was ignored as a bounce.
    pub fn record_tap(&mut self, now: Instant) -> bool {
        if let Some(&last) = self.taps.back() {
            let gap = now.saturating_duration_since(last);
            if gap < DEBOUNCE {
                return false;
            }
            if gap > RESET_GAP {
                // Start a fresh sequence but keep the last tempo until two new taps arrive
                self.taps.clear();
            }
        }
        if self.taps.len() >= MAX_TAPS {
            self.taps.pop_front();
        }
        self.taps.push_back(now);
        self.recalculate();
        true
    }

    fn recalculate(&mut self) {
        if self.taps.len() < 2 {
            return;
        }
        let mut intervals: Vec<f64> = self
            .taps
            .iter()
            .zip(self.taps.iter().skip(1))
            .map(|(a, b)| b.saturating_duration_since(*a).as_secs_f64())
            .collect();
        intervals.sort_by(|a, b| a.total_cmp(b));
        // Median is robust against a single missed or doubled tap
        let mid = intervals.len() / 2;
        let median = if intervals.len() % 2 == 0 {
            (intervals[mid - 1] + intervals[mid]) / 2.0
        } else {
            intervals[mid]
        };
        if median > 0.0 {
            self.current_bpm = (60.0 / median).clamp(MIN_BPM, MAX_BPM);
        }
    }

    pub fn bpm(&self) -> f64 {
        self.current_bpm
    }

    pub fn beat_duration(&self) -> Duration {
        Duration::from_secs_f64(60.0 / self.current_bpm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taps(t: &mut TapTempo, start: Instant, ms: &[u64]) {
        for &m in ms {
            t.record_tap(start + Duration::from_millis(m));
        }
    }

    #[test]
    fn steady_taps() {
        let mut t = TapTempo::new(128.0);
        let s = Instant::now();
        taps(&mut t, s, &[0, 500, 1000, 1500]);
        assert!((t.bpm() - 120.0).abs() < 0.01);
    }

    #[test]
    fn ignores_bounce() {
        let mut t = TapTempo::new(128.0);
        let s = Instant::now();
        taps(&mut t, s, &[0, 500, 505, 1000, 1500]);
        assert!((t.bpm() - 120.0).abs() < 0.01);
    }

    #[test]
    fn keeps_last_tempo_after_pause() {
        let mut t = TapTempo::new(128.0);
        let s = Instant::now();
        taps(&mut t, s, &[0, 400, 800, 1200]);
        let before = t.bpm();
        taps(&mut t, s, &[10_000]);
        assert!((t.bpm() - before).abs() < 0.01);
    }

    #[test]
    fn invalid_fallback_is_safe() {
        assert_eq!(TapTempo::new(0.0).bpm(), MIN_BPM);
        assert_eq!(TapTempo::new(f64::NAN).bpm(), 128.0);
        let _ = TapTempo::new(f64::INFINITY).beat_duration();
    }
}
