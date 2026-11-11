//! Colours and the few glyphs the dashboard is drawn with.
//!
//! A theme is a small palette: surfaces, three weights of text, one accent
//! and four colours that carry meaning. Most are painted over the terminal's
//! own background, so the dashboard looks the same everywhere; `terminal`
//! uses the terminal's own 16 colours instead, and so follows its theme.
//! Terminals without 24-bit colour get the nearest entries of the 256-colour
//! cube.

use ratatui::style::{Color, Modifier, Style};

use crate::harness::Harness;

type Rgb = (u8, u8, u8);

/// The themes on offer, in the order Settings steps through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Violet,
    Blue,
    Green,
    Dark,
    Grey,
    Terminal,
}

impl Scheme {
    pub const ALL: [Scheme; 6] = [
        Scheme::Violet,
        Scheme::Blue,
        Scheme::Green,
        Scheme::Dark,
        Scheme::Grey,
        Scheme::Terminal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scheme::Violet => "violet",
            Scheme::Blue => "blue",
            Scheme::Green => "green",
            Scheme::Dark => "dark",
            Scheme::Grey => "grey",
            Scheme::Terminal => "terminal",
        }
    }

    /// What the theme looks like, in a few words.
    pub fn blurb(self) -> &'static str {
        match self {
            Scheme::Violet => "violet on blue-black",
            Scheme::Blue => "blue on navy",
            Scheme::Green => "green on dark teal",
            Scheme::Dark => "amber on true black",
            Scheme::Grey => "greyscale, no colour at all",
            Scheme::Terminal => "your terminal's own 16 colours",
        }
    }

    /// The theme called `name`; an unknown name gives the first one.
    pub fn parse(name: &str) -> Scheme {
        let name = name.trim().to_lowercase();
        Scheme::ALL
            .into_iter()
            .find(|s| s.name() == name)
            .unwrap_or(Scheme::Violet)
    }

    /// The theme `dir` steps away, wrapping at both ends.
    pub fn step(self, dir: i32) -> Scheme {
        let n = Scheme::ALL.len() as i32;
        let at = Scheme::ALL.iter().position(|s| *s == self).unwrap_or(0) as i32;
        Scheme::ALL[(at + if dir < 0 { -1 } else { 1 }).rem_euclid(n) as usize]
    }
}

struct Palette {
    bg: Rgb,
    panel: Rgb,
    raised: Rgb,
    border: Rgb,
    text: Rgb,
    dim: Rgb,
    faint: Rgb,
    track: Rgb,
    accent: Rgb,
    ok: Rgb,
    warn: Rgb,
    bad: Rgb,
    info: Rgb,
}

const VIOLET: Palette = Palette {
    bg: (14, 16, 22),
    panel: (19, 22, 30),
    raised: (34, 39, 54),
    border: (44, 50, 68),
    text: (214, 218, 228),
    dim: (139, 147, 167),
    faint: (84, 92, 112),
    track: (48, 54, 72),
    accent: (167, 139, 250),
    ok: (107, 207, 142),
    warn: (229, 192, 123),
    bad: (240, 113, 120),
    info: (97, 175, 239),
};

const BLUE: Palette = Palette {
    bg: (10, 15, 26),
    panel: (14, 21, 36),
    raised: (26, 40, 66),
    border: (36, 54, 86),
    text: (212, 222, 238),
    dim: (134, 152, 182),
    faint: (78, 96, 128),
    track: (38, 54, 84),
    accent: (96, 165, 250),
    ok: (110, 207, 151),
    warn: (229, 192, 123),
    bad: (240, 113, 120),
    info: (125, 211, 252),
};

const GREEN: Palette = Palette {
    bg: (10, 18, 17),
    panel: (14, 24, 23),
    raised: (26, 44, 41),
    border: (38, 60, 56),
    text: (212, 226, 221),
    dim: (134, 160, 152),
    faint: (80, 104, 98),
    track: (38, 58, 54),
    accent: (94, 214, 160),
    ok: (134, 214, 110),
    warn: (229, 192, 123),
    bad: (240, 113, 120),
    info: (103, 190, 232),
};

const DARK: Palette = Palette {
    bg: (0, 0, 0),
    panel: (8, 8, 8),
    raised: (36, 36, 36),
    border: (52, 52, 52),
    text: (222, 222, 222),
    dim: (150, 150, 150),
    faint: (96, 96, 96),
    track: (50, 50, 50),
    accent: (240, 178, 76),
    ok: (107, 207, 142),
    warn: (229, 192, 123),
    bad: (240, 113, 120),
    info: (97, 175, 239),
};

// No colour at all: how full a bar is shows as brightness.
const GREY: Palette = Palette {
    bg: (16, 16, 16),
    panel: (22, 22, 22),
    raised: (48, 48, 48),
    border: (64, 64, 64),
    text: (220, 220, 220),
    dim: (150, 150, 150),
    faint: (98, 98, 98),
    track: (56, 56, 56),
    accent: (255, 255, 255),
    ok: (140, 140, 140),
    warn: (196, 196, 196),
    bad: (255, 255, 255),
    info: (176, 176, 176),
};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    scheme: Scheme,
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

/// The closest of the terminal's six bright colours, for an agent's dot.
fn to_ansi((r, g, b): Rgb) -> Color {
    const NAMED: [(Rgb, Color); 6] = [
        ((240, 80, 80), Color::LightRed),
        ((80, 220, 120), Color::LightGreen),
        ((230, 180, 80), Color::LightYellow),
        ((80, 140, 250), Color::LightBlue),
        ((200, 120, 240), Color::LightMagenta),
        ((80, 210, 220), Color::LightCyan),
    ];
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max - min < 40 {
        return Color::Gray;
    }
    let far = |(x, y, z): Rgb| {
        let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
        d(r, x) + d(g, y) + d(b, z)
    };
    NAMED
        .iter()
        .min_by_key(|(rgb, _)| far(*rgb))
        .map(|(_, c)| *c)
        .unwrap_or(Color::Gray)
}

impl Theme {
    pub fn detect(paint_bg: bool, scheme: &str) -> Theme {
        let ct = std::env::var("COLORTERM")
            .unwrap_or_default()
            .to_lowercase();
        Theme {
            scheme: Scheme::parse(scheme),
            truecolor: ct.contains("truecolor") || ct.contains("24bit"),
            paint_bg,
        }
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    fn palette(&self) -> &'static Palette {
        match self.scheme {
            Scheme::Violet | Scheme::Terminal => &VIOLET,
            Scheme::Blue => &BLUE,
            Scheme::Green => &GREEN,
            Scheme::Dark => &DARK,
            Scheme::Grey => &GREY,
        }
    }

    fn rgb(&self, (r, g, b): Rgb) -> Color {
        if self.truecolor {
            Color::Rgb(r, g, b)
        } else {
            to_256(r, g, b)
        }
    }

    /// The palette's colour, or `ansi` on the terminal's own 16 colours.
    fn pick(&self, of: fn(&Palette) -> Rgb, ansi: Color) -> Color {
        if self.scheme == Scheme::Terminal {
            ansi
        } else {
            self.rgb(of(self.palette()))
        }
    }

    // ---- surfaces
    pub fn bg(&self) -> Color {
        if self.paint_bg {
            self.pick(|p| p.bg, Color::Reset)
        } else {
            Color::Reset
        }
    }
    pub fn panel(&self) -> Color {
        if self.paint_bg {
            self.pick(|p| p.panel, Color::Reset)
        } else {
            Color::Reset
        }
    }
    /// A selected row. The terminal's own colours have no shade that is safe
    /// under every text colour, so there the row marker alone shows it.
    pub fn raised(&self) -> Color {
        self.pick(|p| p.raised, Color::Reset)
    }
    pub fn border(&self) -> Color {
        self.pick(|p| p.border, Color::DarkGray)
    }

    // ---- text
    pub fn text(&self) -> Color {
        self.pick(|p| p.text, Color::Reset)
    }
    pub fn dim(&self) -> Color {
        self.pick(|p| p.dim, Color::Gray)
    }
    pub fn faint(&self) -> Color {
        self.pick(|p| p.faint, Color::DarkGray)
    }
    /// The track of an empty bar.
    pub fn track(&self) -> Color {
        self.pick(|p| p.track, Color::DarkGray)
    }
    /// Text drawn on a filled block of the accent colour.
    pub fn on_accent(&self) -> Color {
        self.pick(|p| p.bg, Color::Black)
    }

    // ---- meaning
    pub fn accent(&self) -> Color {
        self.pick(|p| p.accent, Color::Cyan)
    }
    pub fn ok(&self) -> Color {
        self.pick(|p| p.ok, Color::Green)
    }
    pub fn warn(&self) -> Color {
        self.pick(|p| p.warn, Color::Yellow)
    }
    pub fn bad(&self) -> Color {
        self.pick(|p| p.bad, Color::Red)
    }
    pub fn info(&self) -> Color {
        self.pick(|p| p.info, Color::Blue)
    }

    pub fn harness(&self, h: Harness) -> Color {
        let (r, g, b) = h.spec().rgb;
        match self.scheme {
            Scheme::Terminal => to_ansi((r, g, b)),
            // Greyscale keeps each agent's brightness and drops its hue.
            Scheme::Grey => {
                let v = ((r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100) as u8;
                let v = v.clamp(120, 230);
                self.rgb((v, v, v))
            }
            _ => self.rgb((r, g, b)),
        }
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
    fn a_theme_is_found_by_name_and_an_unknown_name_is_the_first() {
        for s in Scheme::ALL {
            assert_eq!(Scheme::parse(s.name()), s);
        }
        assert_eq!(Scheme::parse("Blue "), Scheme::Blue);
        assert_eq!(Scheme::parse("nope"), Scheme::Violet);
        assert_eq!(Scheme::Terminal.step(1), Scheme::Violet);
        assert_eq!(Scheme::Violet.step(-1), Scheme::Terminal);
    }

    #[test]
    fn the_terminal_theme_uses_only_the_terminals_own_colours() {
        let t = Theme::detect(true, "terminal");
        let own = |c: Color| !matches!(c, Color::Rgb(..) | Color::Indexed(_));
        for c in [
            t.bg(),
            t.panel(),
            t.raised(),
            t.border(),
            t.text(),
            t.dim(),
            t.faint(),
            t.track(),
            t.accent(),
            t.ok(),
            t.warn(),
            t.bad(),
            t.info(),
            t.on_accent(),
        ] {
            assert!(own(c), "{c:?}");
        }
        for h in Harness::ALL {
            assert!(own(t.harness(h)), "{h:?}");
        }
    }

    #[test]
    fn greyscale_has_no_hue_anywhere() {
        let t = Theme {
            scheme: Scheme::Grey,
            truecolor: true,
            paint_bg: true,
        };
        let mut all = vec![t.accent(), t.ok(), t.warn(), t.bad(), t.info(), t.text()];
        all.extend(Harness::ALL.into_iter().map(|h| t.harness(h)));
        for c in all {
            match c {
                Color::Rgb(r, g, b) => assert!(r == g && g == b, "{c:?}"),
                other => panic!("{other:?}"),
            }
        }
        // Fuller still reads as louder.
        let v = |c: Color| match c {
            Color::Rgb(r, _, _) => r,
            _ => 0,
        };
        assert!(v(t.severity(10.0)) < v(t.severity(80.0)));
        assert!(v(t.severity(80.0)) < v(t.severity(95.0)));
    }

    #[test]
    fn greys_use_the_grey_ramp_and_colours_use_the_cube() {
        assert!(matches!(to_256(128, 128, 128), Color::Indexed(232..=255)));
        assert_eq!(to_256(0, 0, 0), Color::Indexed(16));
        assert_eq!(to_256(255, 0, 0), Color::Indexed(196));
    }
}
