//! Text for a terminal: fitting to a width, and colour for the plain commands.

use std::io::IsTerminal;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cut `s` to at most `w` cells, ending in `…` when something was cut.
pub fn truncate(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > w - 1 {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

/// Exactly `w` cells: cut or padded on the right.
pub fn fit(s: &str, w: usize) -> String {
    let t = truncate(s, w);
    let pad = w.saturating_sub(width(&t));
    format!("{t}{}", " ".repeat(pad))
}

/// Exactly `w` cells: cut or padded on the left.
pub fn fit_right(s: &str, w: usize) -> String {
    let t = truncate(s, w);
    let pad = w.saturating_sub(width(&t));
    format!("{}{t}", " ".repeat(pad))
}

/// Colour for line-by-line output. Off when the stream is not a terminal or
/// `NO_COLOR` is set, so piped output stays plain.
#[derive(Debug, Clone, Copy)]
pub struct Paint {
    on: bool,
}

impl Paint {
    pub fn stdout() -> Paint {
        Paint::new(std::io::stdout().is_terminal())
    }

    pub fn stderr() -> Paint {
        Paint::new(std::io::stderr().is_terminal())
    }

    pub fn new(terminal: bool) -> Paint {
        Paint {
            on: terminal && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on && !s.is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn ok(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn warn(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn bad(&self, s: &str) -> String {
        self.wrap("31", s)
    }
    pub fn info(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    pub fn accent(&self, s: &str) -> String {
        self.wrap("1;35", s)
    }

    /// Green, yellow or red by how full something is.
    pub fn severity(&self, pct: f64, s: &str) -> String {
        if pct >= 90.0 {
            self.bad(s)
        } else if pct >= 70.0 {
            self.warn(s)
        } else {
            self.ok(s)
        }
    }

    /// A usage bar of `cells` cells.
    pub fn bar(&self, pct: f64, cells: usize) -> String {
        let pct = pct.clamp(0.0, 100.0);
        let mut filled = (pct / 100.0 * cells as f64).round() as usize;
        if pct > 0.0 && filled == 0 {
            filled = 1;
        }
        let filled = filled.min(cells);
        format!(
            "{}{}",
            self.severity(pct, &"━".repeat(filled)),
            self.dim(&"─".repeat(cells - filled))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_cut_and_pad_by_display_width() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit_right("7", 3), "  7");
        assert_eq!(truncate("日本語テキスト", 5), "日本…");
        assert_eq!(truncate("x", 0), "");
    }

    #[test]
    fn paint_is_plain_when_off() {
        let p = Paint::new(false);
        assert_eq!(p.bold("x"), "x");
        assert_eq!(width(&p.bar(50.0, 10)), 10);
        let on = Paint { on: true };
        assert!(on.bad("x").starts_with("\x1b[31m"));
        assert_eq!(on.dim(""), "");
    }
}
