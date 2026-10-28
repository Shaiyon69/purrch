//! Feeding the cat: sips, finished tasks and Focus sessions fill it up, and it slowly gets
//! hungry over the day. A hungry cat turns grumpy; good days in a row earn it a bell, then a scarf.

use crate::store;

pub const FILE: &str = "pet.txt";
/// Feeds in a day that make it a good day.
pub const GOOD_DAY: u32 = 8;
/// Fullness lost per minute: a full cat is hungry again after about 7 hours.
const HUNGER: f32 = 0.002;
/// Below this the cat is grumpy.
const GRUMPY: f32 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Food {
    Sip,
    Task,
    Focus,
}

impl Food {
    fn amount(self) -> f32 {
        match self {
            Food::Sip => 0.15,
            Food::Task => 0.2,
            Food::Focus => 0.25,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Accessory {
    #[default]
    None,
    Bell,
    Scarf,
}

#[derive(Debug, PartialEq)]
pub struct Pet {
    pub day: u32,
    pub fullness: f32,
    pub fed_today: u32,
    /// Good days in a row before today.
    pub streak: u32,
    minute: u16,
}

impl Pet {
    pub fn new(day: u32, minute: u16) -> Pet {
        Pet { day, fullness: 0.6, fed_today: 0, streak: 0, minute }
    }

    /// Moves the clock on: a new day settles yesterday's streak, otherwise the cat gets hungrier.
    // shortcut: any new day counts as the next one, so a skipped day keeps the streak; compare dates if it matters.
    pub fn update(&mut self, day: u32, minute: u16) {
        if day != self.day {
            self.streak = if self.fed_today >= GOOD_DAY { self.streak + 1 } else { 0 };
            (self.day, self.fed_today, self.fullness, self.minute) = (day, 0, 0.6, minute);
            return;
        }
        let gone = minute.saturating_sub(self.minute) as f32;
        self.fullness = (self.fullness - gone * HUNGER).max(0.0);
        self.minute = self.minute.max(minute);
    }

    /// Returns true when this feed makes today a good day.
    pub fn feed(&mut self, food: Food) -> bool {
        self.fullness = (self.fullness + food.amount()).min(1.0);
        self.fed_today += 1;
        self.fed_today == GOOD_DAY
    }

    pub fn grumpy(&self) -> bool {
        self.fullness < GRUMPY
    }

    /// Today counts as soon as it's good, so the reward shows up right away.
    pub fn accessory(&self) -> Accessory {
        match self.streak + (self.fed_today >= GOOD_DAY) as u32 {
            0 => Accessory::None,
            1 | 2 => Accessory::Bell,
            _ => Accessory::Scarf,
        }
    }

    pub fn to_text(&self) -> String {
        format!(
            "day={}\nfullness={:.3}\nfed_today={}\nstreak={}\nminute={}\n",
            self.day, self.fullness, self.fed_today, self.streak, self.minute
        )
    }

    pub fn from_text(text: &str, day: u32, minute: u16) -> Pet {
        let m = store::parse(text);
        let d = Pet::new(day, minute);
        let mut pet = Pet {
            day: store::get(&m, "day", d.day),
            fullness: store::get(&m, "fullness", d.fullness).clamp(0.0, 1.0),
            fed_today: store::get(&m, "fed_today", d.fed_today),
            streak: store::get(&m, "streak", d.streak),
            minute: store::get(&m, "minute", d.minute),
        };
        pet.update(day, minute);
        pet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeding_fills_up_to_full() {
        let mut p = Pet::new(20261028, 600);
        for _ in 0..5 {
            p.feed(Food::Focus);
        }
        assert_eq!(p.fullness, 1.0);
        assert_eq!(p.fed_today, 5);
    }

    #[test]
    fn gets_grumpy_when_left_hungry() {
        let mut p = Pet::new(20261028, 600);
        assert!(!p.grumpy());
        p.update(20261028, 600 + 250);
        assert!(p.grumpy());
        p.feed(Food::Task);
        assert!(!p.grumpy());
    }

    #[test]
    fn a_good_day_earns_a_bell_at_once_then_a_scarf() {
        let mut p = Pet::new(1, 0);
        let good = |p: &mut Pet| (0..GOOD_DAY).map(|_| p.feed(Food::Sip)).filter(|&g| g).count();
        assert_eq!(good(&mut p), 1, "the eighth feed announces the good day once");
        assert_eq!(p.accessory(), Accessory::Bell);
        p.update(2, 0);
        assert_eq!(p.accessory(), Accessory::Bell, "yesterday still counts");
        good(&mut p);
        p.update(3, 0);
        good(&mut p);
        assert_eq!(p.accessory(), Accessory::Scarf);
    }

    #[test]
    fn a_lazy_day_breaks_the_streak() {
        let mut p = Pet::new(1, 0);
        (0..GOOD_DAY).for_each(|_| _ = p.feed(Food::Sip));
        p.update(2, 0);
        p.feed(Food::Sip);
        p.update(3, 0);
        assert_eq!(p.accessory(), Accessory::None);
    }

    #[test]
    fn round_trips_and_catches_up_on_load() {
        let mut p = Pet::new(20261028, 600);
        p.feed(Food::Sip);
        let back = Pet::from_text(&p.to_text(), 20261028, 600);
        assert_eq!(back, p);
        let later = Pet::from_text(&p.to_text(), 20261028, 700);
        assert!(later.fullness < p.fullness);
        let tomorrow = Pet::from_text(&p.to_text(), 20261029, 0);
        assert_eq!(tomorrow.fed_today, 0);
    }
}
