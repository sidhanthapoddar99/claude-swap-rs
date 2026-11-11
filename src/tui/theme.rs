//! Colours and the few glyphs the dashboard is drawn with.
//!
//! One dark palette, painted over the terminal's own background so the
//! dashboard looks the same everywhere. Terminals without 24-bit colour get
//! the nearest entries of the 256-colour cube.

use ratatui::style::{Color, Modifier, Style};

use crate::harness::Harness;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    truecolor: bool,
    /// Paint cswap's own background. Off leaves the terminal's (for
    /// transparent terminals).
    paint_bg: bool,
}

/// Nearest xterm-256 colour for an RGB triple.
fn to_256(r: u8, g: u8, b: u8) -> Color {
    // Greys map better onto the 24-step grey ramp than onto the cube.
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max - min < 12 {
        let v = (r as u16 + g as u16 + b as u16) / 3;
        if v < 8 {
            return Color::Indexed(16);
        }
        if v > 238 {
            return Color::Indexed(231);
        }
        return Color::Indexed(232 + ((v - 8) / 10).min(23) as u8);
    }
    let c = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            ((v as u16 - 35) / 40).min(5) as u8
        }
    };
    Color::Indexed(16 + 36 * c(r) + 6 * c(g) + c(b))
}

impl Theme {
    pub fn detect(paint_bg: bool) -> Theme {
        let ct = std::env::var("COLORTERM")
            .unwrap_or_default()
            .to_lowercase();
        Theme {
            truecolor: ct.contains("truecolor") || ct.contains("24bit"),
            paint_bg,
        }
    }

    pub fn rgb(&self, r: u8, g: u8, b: u8) -> Color {
        if self.truecolor {
            Color::Rgb(r, g, b)
        } else {
            to_256(r, g, b)
        }
    }

    // ---- surfaces
    pub fn bg(&self) -> Color {
        if self.paint_bg {
            self.rgb(14, 16, 22)
        } else {
            Color::Reset
        }
    }
    pub fn panel(&self) -> Color {
        if self.paint_bg {
            self.rgb(19, 22, 30)
        } else {
            Color::Reset
        }
    }
    /// A selected row, a pressed chip.
    pub fn raised(&self) -> Color {
        self.rgb(34, 39, 54)
    }
    pub fn border(&self) -> Color {
        self.rgb(44, 50, 68)
    }

    // ---- text
    pub fn text(&self) -> Color {
        self.rgb(214, 218, 228)
    }
    pub fn dim(&self) -> Color {
        self.rgb(139, 147, 167)
    }
    pub fn faint(&self) -> Color {
        self.rgb(84, 92, 112)
    }
    /// The track of an empty bar.
    pub fn track(&self) -> Color {
        self.rgb(48, 54, 72)
    }

    // ---- meaning
    pub fn accent(&self) -> Color {
        self.rgb(167, 139, 250)
    }
    pub fn ok(&self) -> Color {
        self.rgb(107, 207, 142)
    }
    pub fn warn(&self) -> Color {
        self.rgb(229, 192, 123)
    }
    pub fn bad(&self) -> Color {
        self.rgb(240, 113, 120)
    }
    pub fn info(&self) -> Color {
        self.rgb(97, 175, 239)
    }

    pub fn harness(&self, h: Harness) -> Color {
        let (r, g, b) = h.spec().rgb;
        self.rgb(r, g, b)
    }

    /// Green below 70%, amber below 90%, red above.
    pub fn severity(&self, pct: f64) -> Color {
        if pct < 70.0 {
            self.ok()
        } else if pct < 90.0 {
            self.warn()
        } else {
            self.bad()
        }
    }

    // ---- ready-made styles
    pub fn base(&self) -> Style {
        Style::default().fg(self.text()).bg(self.bg())
    }
    pub fn s_text(&self) -> Style {
        Style::default().fg(self.text())
    }
    pub fn s_dim(&self) -> Style {
        Style::default().fg(self.dim())
    }
    pub fn s_faint(&self) -> Style {
        Style::default().fg(self.faint())
    }
    pub fn s_bold(&self) -> Style {
        Style::default()
            .fg(self.text())
            .add_modifier(Modifier::BOLD)
    }
    pub fn s_accent(&self) -> Style {
        Style::default().fg(self.accent())
    }
    pub fn s(&self, c: Color) -> Style {
        Style::default().fg(c)
    }
}

pub const DOT: &str = "●";
pub const RING: &str = "○";
pub const STAR: &str = "★";
pub const CHECK: &str = "✓";
pub const CROSS: &str = "✗";
pub const WARN: &str = "▲";
pub const ARROW: &str = "▸";
pub const BAR_FULL: &str = "━";
pub const BAR_PACE: &str = "╋";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greys_use_the_grey_ramp_and_colours_use_the_cube() {
        assert!(matches!(to_256(128, 128, 128), Color::Indexed(232..=255)));
        assert_eq!(to_256(0, 0, 0), Color::Indexed(16));
        assert_eq!(to_256(255, 0, 0), Color::Indexed(196));
    }
}
