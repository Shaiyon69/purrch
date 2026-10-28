//! Turns the output's peak level into beats: a hit is a peak well above the recent average,
//! that comes after the level dipped back, with a short gap so one drum doesn't count twice.

/// Below this the music is too quiet to pulse to.
const FLOOR: f32 = 0.03;
/// How far above the running average a peak must jump to count.
const JUMP: f32 = 1.35;
/// About 215 BPM at most.
const GAP: f32 = 0.28;
/// Seconds the running average looks back.
const WINDOW: f32 = 0.5;

#[derive(Default)]
pub struct Beat {
    avg: f32,
    /// Seconds until another beat may count.
    wait: f32,
    /// A sustained loud stretch is one beat; the level has to dip before the next.
    held: bool,
    /// 1.0 on a beat, easing back to 0; flashes the pill's outline.
    pub pulse: f32,
    /// Loudness that jumps up with the music and falls back slowly; the outline's steady glow.
    pub level: f32,
}

impl Beat {
    /// Feeds one peak reading taken `dt` seconds after the last; true on a beat.
    pub fn step(&mut self, peak: f32, dt: f32) -> bool {
        self.wait -= dt;
        self.pulse *= (-dt * 8.0).exp();
        self.level = peak.max(self.level * (-dt * 5.0).exp());
        let loud = peak > FLOOR && peak > self.avg * JUMP;
        let hit = loud && !self.held && self.wait <= 0.0;
        self.held = loud;
        self.avg += (peak - self.avg) * (dt / WINDOW).min(1.0);
        if hit {
            self.wait = GAP;
            self.pulse = 1.0;
        }
        hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    /// Beats counted over `secs` of a signal sampled at 60 fps.
    fn beats(secs: f32, signal: impl Fn(f32) -> f32) -> usize {
        let mut b = Beat::default();
        (0..(secs / DT) as usize).filter(|&i| b.step(signal(i as f32 * DT), DT)).count()
    }

    #[test]
    fn silence_and_steady_tones_never_beat() {
        assert_eq!(beats(5.0, |_| 0.0), 0);
        // A steady level only "beats" once, as it first rises from silence.
        assert_eq!(beats(5.0, |_| 0.5), 1);
    }

    #[test]
    fn kicks_at_120_bpm_beat_twice_a_second() {
        let kick = |t: f32| if t % 0.5 < 0.05 { 0.9 } else { 0.2 };
        let n = beats(4.0, kick);
        assert!((7..=8).contains(&n), "{n} beats");
    }

    #[test]
    fn hits_closer_than_the_gap_count_once() {
        let flutter = |t: f32| if t % 0.1 < 0.03 { 0.9 } else { 0.1 };
        assert!(beats(2.0, flutter) <= (2.0 / GAP) as usize + 1);
    }

    #[test]
    fn level_rises_at_once_and_falls_slowly() {
        let mut b = Beat::default();
        b.step(0.8, DT);
        assert_eq!(b.level, 0.8);
        b.step(0.0, DT);
        assert!(b.level > 0.7, "one quiet frame barely dims it");
        for _ in 0..60 {
            b.step(0.0, DT);
        }
        assert!(b.level < 0.01);
    }

    #[test]
    fn pulse_jumps_on_a_beat_then_fades() {
        let mut b = Beat::default();
        assert!(b.step(0.9, DT));
        assert_eq!(b.pulse, 1.0);
        for _ in 0..30 {
            b.step(0.2, DT);
        }
        assert!(b.pulse < 0.05);
    }
}
