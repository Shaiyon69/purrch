use crate::store;

pub const FILE: &str = "todo.txt";

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub title: String,
    pub done: bool,
    /// Optional reminder, minutes since midnight. Written as "@14:30" at the end of the title.
    pub at: Option<u16>,
    /// Day (yyyymmdd) the reminder last fired, so it fires once per day.
    pub fired_day: u32,
}

impl Task {
    /// "Call mom @18:00" becomes title "Call mom" with a reminder at 18:00.
    pub fn parse(input: &str) -> Option<Task> {
        let input = input.trim();
        let (title, at) = match input.rsplit_once(" @") {
            Some((t, time)) => match parse_time(time) {
                Some(m) => (t.trim(), Some(m)),
                None => (input, None),
            },
            None => (input, None),
        };
        (!title.is_empty()).then(|| Task { title: title.to_string(), done: false, at, fired_day: 0 })
    }
}

fn parse_time(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

#[derive(Default)]
pub struct Todo {
    pub tasks: Vec<Task>,
}

impl Todo {
    pub fn load() -> Todo {
        let text = std::fs::read_to_string(store::dir().join(FILE)).unwrap_or_default();
        Todo::from_text(&text)
    }

    pub fn save(&self) {
        let _ = store::save(FILE, &self.to_text());
    }

    /// Saves typed text as a new task (`index` None) or over task `index`, where empty text
    /// deletes it. Returns false when nothing changed.
    pub fn write(&mut self, index: Option<usize>, text: &str) -> bool {
        match (index, Task::parse(text)) {
            (None, Some(task)) => self.tasks.push(task),
            (Some(i), Some(task)) if i < self.tasks.len() => {
                let old = &mut self.tasks[i];
                // A changed reminder time may fire again today.
                let fired_day = if old.at == task.at { old.fired_day } else { 0 };
                *old = Task { done: old.done, fired_day, ..task };
            }
            (Some(i), None) if i < self.tasks.len() => {
                self.tasks.remove(i);
            }
            _ => return false,
        }
        true
    }

    pub fn from_text(text: &str) -> Todo {
        let tasks = text
            .lines()
            .filter_map(|line| {
                let mut f = line.splitn(4, '\t');
                let done = f.next()? == "1";
                let at = f.next()?.parse().ok();
                let fired_day = f.next()?.parse().unwrap_or(0);
                let title = f.next()?.to_string();
                (!title.is_empty()).then_some(Task { title, done, at, fired_day })
            })
            .collect();
        Todo { tasks }
    }

    pub fn to_text(&self) -> String {
        self.tasks
            .iter()
            .map(|t| {
                let at = t.at.map_or(String::new(), |m| m.to_string());
                // Tabs and newlines would break the line format.
                let title = t.title.replace(['\t', '\n', '\r'], " ");
                format!("{}\t{}\t{}\t{}\n", t.done as u8, at, t.fired_day, title)
            })
            .collect()
    }

    /// Returns the title of a task whose reminder time has come, at most once per day.
    pub fn due(&mut self, day: u32, now: u16) -> Option<String> {
        let t = self
            .tasks
            .iter_mut()
            .find(|t| !t.done && t.fired_day != day && t.at.is_some_and(|at| at <= now && now < at + 15))?;
        t.fired_day = day;
        Some(t.title.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_new_edited_and_cleared_tasks() {
        let mut todo = Todo::default();
        assert!(todo.write(None, "Call mom @18:00"));
        assert!(!todo.write(None, "   "));
        todo.tasks[0].done = true;
        todo.tasks[0].fired_day = 20261026;
        assert!(todo.write(Some(0), "Call dad @18:00"));
        let t = &todo.tasks[0];
        assert_eq!((t.title.as_str(), t.done, t.at, t.fired_day), ("Call dad", true, Some(18 * 60), 20261026));
        assert!(todo.write(Some(0), "Call dad @19:00"));
        assert_eq!(todo.tasks[0].fired_day, 0);
        assert!(!todo.write(Some(5), "nope"));
        assert!(todo.write(Some(0), ""));
        assert!(todo.tasks.is_empty());
    }

    #[test]
    fn parses_reminder_suffix() {
        let t = Task::parse("Call mom @18:05").unwrap();
        assert_eq!((t.title.as_str(), t.at), ("Call mom", Some(18 * 60 + 5)));
        let t = Task::parse("Email me@25:00").unwrap();
        assert_eq!((t.title.as_str(), t.at), ("Email me@25:00", None));
        assert_eq!(Task::parse("   "), None);
    }

    #[test]
    fn round_trips_and_sanitizes() {
        let mut todo = Todo::default();
        todo.tasks.push(Task::parse("a\tb @09:00").unwrap());
        todo.tasks.push(Task { done: true, ..Task::parse("done thing").unwrap() });
        let back = Todo::from_text(&todo.to_text());
        assert_eq!(back.tasks[0].title, "a b");
        assert_eq!(back.tasks[0].at, Some(540));
        assert!(back.tasks[1].done);
    }

    #[test]
    fn reminder_fires_once_per_day() {
        let mut todo = Todo { tasks: vec![Task::parse("stretch @10:00").unwrap()] };
        assert_eq!(todo.due(1, 599), None);
        assert_eq!(todo.due(1, 600).as_deref(), Some("stretch"));
        assert_eq!(todo.due(1, 601), None);
        assert_eq!(todo.due(2, 601).as_deref(), Some("stretch"));
    }
}
