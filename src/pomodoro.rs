use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Focus,
    Short,
    Long,
}

impl Mode {
    pub fn length(self) -> Duration {
        Duration::from_secs(match self {
            Mode::Focus => 25 * 60,
            Mode::Short => 5 * 60,
            Mode::Long => 15 * 60,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Focus => "Focus",
            Mode::Short => "Short break",
            Mode::Long => "Long break",
        }
    }
}

pub struct Pomodoro {
    pub mode: Mode,
    /// Focus sessions finished since the last long break.
    pub cycles: u8,
    left: Duration,
    deadline: Option<Instant>,
}

impl Default for Pomodoro {
    fn default() -> Self {
        Pomodoro { mode: Mode::Focus, cycles: 0, left: Mode::Focus.length(), deadline: None }
    }
}

impl Pomodoro {
    pub fn running(&self) -> bool {
        self.deadline.is_some()
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        self.deadline.map_or(self.left, |d| d.saturating_duration_since(now))
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.left = mode.length();
        self.deadline = None;
    }

    pub fn toggle(&mut self, now: Instant) {
        match self.deadline.take() {
            Some(d) => self.left = d.saturating_duration_since(now),
            None => self.deadline = Some(now + self.left),
        }
    }

    pub fn reset(&mut self) {
        self.set_mode(self.mode);
    }

    /// Returns the mode that comes next when the running timer finishes.
    /// The next timer is ready but not started, so a break never starts by itself.
    pub fn tick(&mut self, now: Instant) -> Option<Mode> {
        if self.deadline? > now {
            return None;
        }
        let next = if self.mode == Mode::Focus {
            self.cycles += 1;
            if self.cycles >= 4 {
                self.cycles = 0;
                Mode::Long
            } else {
                Mode::Short
            }
        } else {
            Mode::Focus
        };
        self.set_mode(next);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish(p: &mut Pomodoro, now: &mut Instant) -> Option<Mode> {
        p.toggle(*now);
        *now += p.mode.length();
        p.tick(*now)
    }

    #[test]
    fn every_fourth_focus_earns_a_long_break() {
        let mut p = Pomodoro::default();
        let mut now = Instant::now();
        let mut breaks = vec![];
        for _ in 0..4 {
            breaks.push(finish(&mut p, &mut now).unwrap());
            assert_eq!(finish(&mut p, &mut now), Some(Mode::Focus));
        }
        assert_eq!(breaks, [Mode::Short, Mode::Short, Mode::Short, Mode::Long]);
        assert_eq!(p.cycles, 0);
    }

    #[test]
    fn pause_keeps_remaining_time() {
        let mut p = Pomodoro::default();
        let now = Instant::now();
        p.toggle(now);
        p.toggle(now + Duration::from_secs(60));
        assert!(!p.running());
        assert_eq!(p.remaining(now + Duration::from_secs(999)), Duration::from_secs(24 * 60));
        assert_eq!(p.tick(now + Duration::from_secs(9999)), None);
    }
}
