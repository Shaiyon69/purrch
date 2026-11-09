//! Feeding the cat: sips, finished tasks and Focus sessions fill it up, and it slowly gets
//! hungry over the day. A hungry cat turns grumpy.

use crate::store;

pub const FILE: &str = "pet.txt";
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

#[derive(Debug, PartialEq)]
pub struct Pet {
    pub day: u32,
    pub fullness: f32,
    minute: u16,
}

impl Pet {
    pub fn new(day: u32, minute: u16) -> Pet {
        Pet { day, fullness: 0.6, minute }
    }

    /// Moves the clock on: a new day starts the cat at its usual fullness, otherwise it gets hungrier.
    pub fn update(&mut self, day: u32, minute: u16) {
        if day != self.day {
            (self.day, self.fullness, self.minute) = (day, 0.6, minute);
            return;
        }
        let gone = minute.saturating_sub(self.minute) as f32;
        self.fullness = (self.fullness - gone * HUNGER).max(0.0);
        self.minute = self.minute.max(minute);
    }

    pub fn feed(&mut self, food: Food) {
        self.fullness = (self.fullness + food.amount()).min(1.0);
    }

    pub fn grumpy(&self) -> bool {
        self.fullness < GRUMPY
    }

    pub fn to_text(&self) -> String {
        format!("day={}\nfullness={:.3}\nminute={}\n", self.day, self.fullness, self.minute)
    }

    /// Older files also hold `fed_today` and `streak`; they are ignored.
    pub fn from_text(text: &str, day: u32, minute: u16) -> Pet {
        let m = store::parse(text);
        let d = Pet::new(day, minute);
        let mut pet = Pet {
            day: store::get(&m, "day", d.day),
            fullness: store::get(&m, "fullness", d.fullness).clamp(0.0, 1.0),
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
    fn round_trips_and_catches_up_on_load() {
        let mut p = Pet::new(20261028, 600);
        p.feed(Food::Sip);
        let back = Pet::from_text(&p.to_text(), 20261028, 600);
        assert_eq!(back, p);
        let later = Pet::from_text(&p.to_text(), 20261028, 700);
        assert!(later.fullness < p.fullness);
        let old = Pet::from_text("day=20261028\nfullness=0.9\nfed_today=8\nstreak=4\nminute=600\n", 20261028, 600);
        assert_eq!(old.fullness, 0.9);
    }
}
