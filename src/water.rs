use crate::store;

pub const FILE: &str = "water.txt";
pub const GOALS: [u32; 4] = [1500, 2000, 2500, 3000];

#[derive(Debug, PartialEq)]
pub struct Schedule {
    pub count: u32,
    /// Minutes between reminders.
    pub interval: f32,
    pub ml_each: u32,
    pub per_bottle: u32,
    pub refills: u32,
}

/// Spread the daily goal into sips of ~25 ml or more, at most one every 15 minutes.
pub fn schedule(bottle: u32, goal: u32, start: u16, end: u16) -> Schedule {
    let window = end.saturating_sub(start).max(60) as f32;
    let goal_f = goal as f32;
    let count = (goal_f / 25.0)
        .round()
        .clamp(1.0, 64.0)
        .min((window / 15.0).floor().max(1.0));
    let ml_each = (goal_f / count).round().max(1.0) as u32;
    Schedule {
        count: count as u32,
        interval: window / count,
        ml_each,
        per_bottle: (bottle as f32 / ml_each as f32).ceil().max(1.0) as u32,
        refills: (goal_f / bottle.max(1) as f32).ceil().max(1.0) as u32 - 1,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Prompt {
    Drink,
    Refill,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Water {
    pub bottle: u32,
    pub goal: u32,
    /// Minutes since midnight.
    pub start: u16,
    pub end: u16,
    /// yyyymmdd the counters below belong to.
    pub day: u32,
    pub taken: u32,
    pub in_bottle: u32,
    /// Bit i set = reminder slot i already shown today.
    pub fired: u64,
    pub hours: [u32; 24],
    pub on: bool,
    pub snooze_until: Option<u16>,
}

impl Default for Water {
    fn default() -> Self {
        Water {
            bottle: 500,
            goal: 2000,
            start: 8 * 60,
            end: 17 * 60,
            day: 0,
            taken: 0,
            in_bottle: 500,
            fired: 0,
            hours: [0; 24],
            on: true,
            snooze_until: None,
        }
    }
}

impl Water {
    pub fn schedule(&self) -> Schedule {
        schedule(self.bottle, self.goal, self.start, self.end)
    }

    pub fn new_day(&mut self, day: u32) {
        if self.day != day {
            self.day = day;
            self.reset();
        }
    }

    pub fn reset(&mut self) {
        self.taken = 0;
        self.hours = [0; 24];
        self.fired = 0;
        self.snooze_until = None;
        self.in_bottle = self.bottle;
    }

    /// Called every tick. Returns a prompt when a reminder (or snooze) comes due.
    pub fn check(&mut self, now: f32) -> Option<Prompt> {
        if !self.on || self.taken >= self.goal {
            return None;
        }
        let s = self.schedule();
        let mut due = false;
        if let Some(t) = self.snooze_until {
            let t = t as f32;
            if now >= t {
                self.snooze_until = None;
                due = now < t + 3.0;
            }
        }
        for i in 0..s.count.min(64) {
            let at = self.start as f32 + (i as f32 + 0.5) * s.interval;
            if self.fired & (1 << i) == 0 && at <= now && now < at + 2.0 {
                self.fired |= 1 << i;
                due = true;
            }
        }
        due.then_some({
            if (self.in_bottle as f32) < s.ml_each as f32 * 0.75 {
                Prompt::Refill
            } else {
                Prompt::Drink
            }
        })
    }

    /// Minutes since midnight of the next reminder still to come today.
    pub fn next_at(&self, now: f32) -> Option<f32> {
        let s = self.schedule();
        (0..s.count.min(64))
            .map(|i| self.start as f32 + (i as f32 + 0.5) * s.interval)
            .find(|&at| at > now)
    }

    pub fn sip(&mut self, hour: usize) {
        let ml = self.schedule().ml_each;
        self.taken += ml;
        self.hours[hour.min(23)] += ml;
        self.in_bottle = self.in_bottle.saturating_sub(ml);
    }

    pub fn refill(&mut self) {
        self.in_bottle = self.bottle;
    }

    pub fn snooze(&mut self, now: u16) {
        self.snooze_until = Some((now + 10).min(1439));
    }

    pub fn step_bottle(&mut self, delta: i32) {
        self.bottle = (self.bottle as i32 + delta).clamp(100, 2000) as u32;
        self.settings_changed();
    }

    pub fn set_goal(&mut self, goal: u32) {
        self.goal = goal;
        self.settings_changed();
    }

    pub fn step_start(&mut self, delta: i32) {
        self.start = (self.start as i32 + delta).clamp(0, 23 * 60) as u16;
        self.settings_changed();
    }

    pub fn step_end(&mut self, delta: i32) {
        self.end = (self.end as i32 + delta).clamp(0, 24 * 60) as u16;
        self.settings_changed();
    }

    fn settings_changed(&mut self) {
        self.end = self.end.max(self.start + 60).min(24 * 60);
        self.in_bottle = self.in_bottle.min(self.bottle);
        self.fired = 0;
        self.snooze_until = None;
    }

    pub fn to_text(&self) -> String {
        let hours: Vec<String> = self.hours.iter().map(u32::to_string).collect();
        format!(
            "bottle={}\ngoal={}\nstart={}\nend={}\nday={}\ntaken={}\nin_bottle={}\nfired={}\nhours={}\non={}\n",
            self.bottle,
            self.goal,
            self.start,
            self.end,
            self.day,
            self.taken,
            self.in_bottle,
            self.fired,
            hours.join(","),
            self.on as u8
        )
    }

    pub fn from_text(text: &str) -> Water {
        let m = store::parse(text);
        let d = Water::default();
        let mut hours = [0; 24];
        if let Some(h) = m.get("hours") {
            for (slot, v) in hours.iter_mut().zip(h.split(',')) {
                *slot = v.parse().unwrap_or(0);
            }
        }
        let mut w = Water {
            bottle: store::get(&m, "bottle", d.bottle),
            goal: store::get(&m, "goal", d.goal),
            start: store::get(&m, "start", d.start),
            end: store::get(&m, "end", d.end),
            day: store::get(&m, "day", 0),
            taken: store::get(&m, "taken", 0),
            in_bottle: store::get(&m, "in_bottle", d.in_bottle),
            fired: store::get(&m, "fired", 0),
            hours,
            on: store::get(&m, "on", 1u8) != 0,
            snooze_until: None,
        };
        w.settings_changed_keep_progress();
        w
    }

    /// Clamp values from disk without clearing today's reminder bits.
    fn settings_changed_keep_progress(&mut self) {
        self.bottle = self.bottle.clamp(100, 2000);
        self.start = self.start.min(23 * 60);
        self.end = self.end.max(self.start + 60).min(24 * 60);
        self.in_bottle = self.in_bottle.min(self.bottle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_schedule() {
        assert_eq!(
            schedule(500, 2000, 480, 1020),
            Schedule { count: 36, interval: 15.0, ml_each: 56, per_bottle: 9, refills: 3 }
        );
    }

    #[test]
    fn short_window_still_gets_one_hour() {
        let s = schedule(500, 1500, 600, 600);
        assert_eq!(s.count, 4);
        assert_eq!(s.interval, 15.0);
    }

    #[test]
    fn each_slot_fires_once_inside_its_window() {
        let mut w = Water::default();
        // First slot is at 480 + 7.5.
        assert_eq!(w.check(487.0), None);
        assert_eq!(w.check(487.5), Some(Prompt::Drink));
        assert_eq!(w.check(488.0), None);
        // Missed by more than 2 minutes: skipped, not nagged later.
        assert_eq!(w.check(505.0), None);
    }

    #[test]
    fn empty_bottle_asks_for_refill() {
        let mut w = Water { in_bottle: 10, ..Water::default() };
        assert_eq!(w.check(487.5), Some(Prompt::Refill));
    }

    #[test]
    fn snooze_fires_ten_minutes_later() {
        let mut w = Water::default();
        w.snooze(600);
        assert_eq!(w.check(605.0), None);
        assert_eq!(w.check(610.2), Some(Prompt::Drink));
        assert_eq!(w.snooze_until, None);
    }

    #[test]
    fn goal_reached_stops_reminders() {
        let mut w = Water { taken: 2000, ..Water::default() };
        assert_eq!(w.check(487.5), None);
    }

    #[test]
    fn sip_updates_counters() {
        let mut w = Water::default();
        w.sip(9);
        assert_eq!((w.taken, w.hours[9], w.in_bottle), (56, 56, 444));
    }

    #[test]
    fn settings_are_clamped() {
        let mut w = Water::default();
        w.step_bottle(-10_000);
        assert_eq!(w.bottle, 100);
        w.step_start(10_000);
        assert_eq!((w.start, w.end), (1380, 1440));
    }

    #[test]
    fn round_trips_through_text() {
        let mut w = Water::default();
        w.day = 20261025;
        w.sip(14);
        w.fired = 0b101;
        assert_eq!(Water::from_text(&w.to_text()), w);
    }
}
