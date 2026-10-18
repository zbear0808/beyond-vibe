use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct TapTempo {
    taps: VecDeque<Instant>,
    max_history: usize,
    pause_reset_duration: Duration,
    fallback_bpm: f64,
}

impl TapTempo {
    pub fn new(fallback_bpm: f64) -> Self {
        Self {
            taps: VecDeque::with_capacity(4),
            max_history: 4,
            pause_reset_duration: Duration::from_millis(2500), // >2.5s gap resets (<24 BPM)
            fallback_bpm,
        }
    }

    pub fn record_tap(&mut self, now: Instant) {
        if let Some(&last) = self.taps.back() {
            if now.duration_since(last) > self.pause_reset_duration {
                self.taps.clear();
            }
        }
        if self.taps.len() >= self.max_history {
            self.taps.pop_front();
        }
        self.taps.push_back(now);
    }

    pub fn bpm(&self) -> f64 {
        if self.taps.len() < 2 {
            return self.fallback_bpm;
        }

        let mut total_duration = Duration::ZERO;
        let intervals = self.taps.len() - 1;

        for i in 0..intervals {
            total_duration += self.taps[i + 1].duration_since(self.taps[i]);
        }

        let avg_secs = total_duration.as_secs_f64() / (intervals as f64);
        if avg_secs <= 0.0 {
            return self.fallback_bpm;
        }

        let calculated_bpm = 60.0 / avg_secs;
        // Clamp to sensible range
        calculated_bpm.clamp(40.0, 300.0)
    }

    pub fn beat_duration(&self) -> Duration {
        let bpm = self.bpm();
        Duration::from_secs_f64(60.0 / bpm)
    }
}

