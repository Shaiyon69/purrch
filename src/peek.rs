//! What the pill says about freshly copied text: a colour swatch for a hex code, the site
//! for a link, and the answer for a sum. Anything else is left alone.

#[derive(Debug, PartialEq)]
pub enum Peek {
    Color(u8, u8, u8),
    Link(String),
    Sum(String),
}

pub fn classify(text: &str) -> Option<Peek> {
    let t = text.trim();
    if t.is_empty() || t.len() > 200 || t.contains('\n') {
        return None;
    }
    hex_code(t).or_else(|| link(t)).or_else(|| sum(t))
}

/// "#FF8800", the form both the clipboard peek and the colour picker show and copy.
pub fn rgb_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// "#f80" or "#ff8800".
fn hex_code(t: &str) -> Option<Peek> {
    let h = t.strip_prefix('#')?;
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |s: &str| u8::from_str_radix(s, 16).ok();
    match h.len() {
        3 => {
            let d: Vec<u8> = h.chars().map(|c| c.to_digit(16).unwrap() as u8 * 17).collect();
            Some(Peek::Color(d[0], d[1], d[2]))
        }
        6 => Some(Peek::Color(v(&h[0..2])?, v(&h[2..4])?, v(&h[4..6])?)),
        _ => None,
    }
}

/// The host only: paths and query strings can carry tokens, so they never reach the screen.
fn link(t: &str) -> Option<Peek> {
    let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?.split(':').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (host.contains('.') && !t.contains(' ')).then(|| Peek::Link(host.to_string()))
}

/// Plain arithmetic like "12*7+3". A bare "-" needs spaces around it, so dates
/// ("2026-10-28") and phone numbers ("555-1234") don't turn into subtractions.
fn sum(t: &str) -> Option<Peek> {
    if !t.chars().all(|c| c.is_ascii_digit() || " +-*/x×().,".contains(c)) {
        return None;
    }
    let ops = t.contains(['+', '*', '/', 'x', '×']) || t.contains(" - ");
    if !ops || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let expr: Vec<char> = t.replace(['x', '×'], "*").replace(',', "").chars().filter(|c| !c.is_whitespace()).collect();
    let mut p = Parser { s: &expr, i: 0 };
    let v = p.expr()?;
    (p.i == expr.len() && v.is_finite()).then(|| Peek::Sum(format_num(v)))
}

fn format_num(v: f64) -> String {
    let s = format!("{:.6}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Recursive descent over + - * / and brackets, with the usual precedence.
struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.i += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.atom()?;
        while let Some(op @ ('*' | '/')) = self.peek() {
            self.i += 1;
            let r = self.atom()?;
            v = if op == '*' { v * r } else { v / r };
        }
        Some(v)
    }

    fn atom(&mut self) -> Option<f64> {
        match self.peek()? {
            '-' => {
                self.i += 1;
                Some(-self.atom()?)
            }
            '(' => {
                self.i += 1;
                let v = self.expr()?;
                (self.peek()? == ')').then(|| self.i += 1)?;
                Some(v)
            }
            _ => {
                let start = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                    self.i += 1;
                }
                self.s[start..self.i].iter().collect::<String>().parse().ok()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        assert_eq!(rgb_hex(255, 136, 0), "#FF8800");
        assert_eq!(classify(&rgb_hex(1, 2, 3)), Some(Peek::Color(1, 2, 3)));
    }

    #[test]
    fn hex_colours() {
        assert_eq!(classify("#ff8800"), Some(Peek::Color(255, 136, 0)));
        assert_eq!(classify("  #F80 "), Some(Peek::Color(255, 136, 0)));
        assert_eq!(classify("#ff88"), None);
        assert_eq!(classify("#zzzzzz"), None);
    }

    #[test]
    fn links_show_only_the_host() {
        assert_eq!(classify("https://www.github.com/a/b?token=secret"), Some(Peek::Link("github.com".into())));
        assert_eq!(classify("http://user:pw@example.org:8080/x"), Some(Peek::Link("example.org".into())));
        assert_eq!(classify("https://localhost"), None);
        assert_eq!(classify("see https://x.com"), None);
    }

    #[test]
    fn sums_with_precedence() {
        assert_eq!(classify("12*7+3"), Some(Peek::Sum("87".into())));
        assert_eq!(classify("(2 + 3) x 4"), Some(Peek::Sum("20".into())));
        assert_eq!(classify("10 / 4"), Some(Peek::Sum("2.5".into())));
        assert_eq!(classify("1,000 + 1"), Some(Peek::Sum("1001".into())));
        assert_eq!(classify("100 - 1"), Some(Peek::Sum("99".into())));
    }

    #[test]
    fn plain_text_dates_and_numbers_are_ignored() {
        assert_eq!(classify("2026-10-28"), None);
        assert_eq!(classify("555-1234"), None);
        assert_eq!(classify("42"), None);
        assert_eq!(classify("hello world"), None);
        assert_eq!(classify("1/0"), None);
        assert_eq!(classify("2 +"), None);
        assert_eq!(classify("line one\nline two"), None);
    }
}
