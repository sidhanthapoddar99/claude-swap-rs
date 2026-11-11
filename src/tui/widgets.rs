//! Building blocks shared by every page: text fitting, usage bars, cards, key
//! chips and the click map.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph};
use ratatui::Frame;

use super::app::Action;
use super::theme::{Theme, BAR_FULL, BAR_PACE};

pub use crate::text::{fit, fit_right, truncate, width};

pub fn line_width(line: &Line) -> usize {
    line.spans.iter().map(|s| width(&s.content)).sum()
}

// ----------------------------------------------------------------- usage bar

/// A usage bar of `cells` cells. `pace` (0–100) marks how much of the window's
/// time has passed, so usage ahead of or behind the clock shows at a glance.
pub fn bar<'a>(t: &Theme, pct: f64, cells: usize, pace: Option<f64>) -> Vec<Span<'a>> {
    let cells = cells.max(1);
    let pct = pct.clamp(0.0, 100.0);
    let mut filled = (pct / 100.0 * cells as f64).round() as usize;
    if pct > 0.0 && filled == 0 {
        filled = 1;
    }
    let filled = filled.min(cells);
    let mark = pace.map(|p| ((p.clamp(0.0, 100.0) / 100.0 * cells as f64) as usize).min(cells - 1));
    let fill = Style::default().fg(t.severity(pct));
    let track = Style::default().fg(t.track());

    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_style = fill;
    let flush = |spans: &mut Vec<Span<'a>>, run: &mut String, style: Style| {
        if !run.is_empty() {
            spans.push(Span::styled(std::mem::take(run), style));
        }
    };
    for i in 0..cells {
        let style = if i < filled { fill } else { track };
        if Some(i) == mark {
            flush(&mut spans, &mut run, run_style);
            spans.push(Span::styled(
                BAR_PACE,
                Style::default().fg(t.text()).add_modifier(Modifier::BOLD),
            ));
            run_style = style;
            continue;
        }
        if style != run_style {
            flush(&mut spans, &mut run, run_style);
            run_style = style;
        }
        run.push_str(BAR_FULL);
    }
    flush(&mut spans, &mut run, run_style);
    spans
}

/// `41%`, right-aligned in four cells, coloured by severity.
pub fn pct_span<'a>(t: &Theme, pct: f64) -> Span<'a> {
    Span::styled(
        format!("{:>3.0}%", pct.clamp(0.0, 999.0)),
        Style::default()
            .fg(t.severity(pct))
            .add_modifier(Modifier::BOLD),
    )
}

/// An eight-level sparkline of `values` (0–100), one cell per value.
pub fn spark(values: &[f64]) -> String {
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    values
        .iter()
        .map(|v| LEVELS[((v.clamp(0.0, 100.0) / 100.0 * 7.0).round() as usize).min(7)])
        .collect()
}

// --------------------------------------------------------------------- cards

/// A rounded panel with a title. Returns the area inside it.
pub fn card(f: &mut Frame, area: Rect, title: &str, t: &Theme, accent: Option<Color>) -> Rect {
    let border = accent.unwrap_or_else(|| t.border());
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border).bg(t.panel()))
        .style(Style::default().bg(t.panel()).fg(t.text()))
        .padding(Padding::horizontal(1));
    if !title.is_empty() {
        block = block.title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                title.to_string(),
                Style::default()
                    .fg(accent.unwrap_or_else(|| t.dim()))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// Add a right-aligned note to the top border of a card drawn at `area`.
pub fn card_note(f: &mut Frame, area: Rect, note: Line, t: &Theme) {
    if area.width < 8 || area.height == 0 {
        return;
    }
    let w = (line_width(&note) as u16 + 2).min(area.width.saturating_sub(4));
    let rect = Rect::new(area.x + area.width - w - 2, area.y, w, 1);
    let mut spans = vec![Span::raw(" ")];
    spans.extend(note.spans);
    spans.push(Span::raw(" "));
    f.render_widget(
        Paragraph::new(Line::from(spans))
            .style(Style::default().bg(t.panel()).fg(t.dim()))
            .alignment(Alignment::Right),
        rect,
    );
}

/// `label  value` with the label dimmed and padded to `label_w`.
pub fn kv<'a>(t: &Theme, label: &str, label_w: usize, value: Vec<Span<'a>>) -> Line<'a> {
    let mut spans = vec![Span::styled(fit(label, label_w), t.s_dim())];
    spans.extend(value);
    Line::from(spans)
}

/// Break styled text into lines of at most `w` cells, at spaces, keeping each
/// word's style.
pub fn wrap_spans(spans: Vec<Span<'static>>, w: usize) -> Vec<Vec<Span<'static>>> {
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut used = 0;
    for span in spans {
        for (i, word) in span.content.split(' ').enumerate() {
            let piece = if i > 0 {
                format!(" {word}")
            } else {
                word.to_string()
            };
            if piece.is_empty() {
                continue;
            }
            let pw = width(&piece);
            if used > 0 && used + pw > w {
                lines.push(Vec::new());
                let piece = piece.trim_start().to_string();
                used = width(&piece);
                lines
                    .last_mut()
                    .expect("just pushed")
                    .push(Span::styled(piece, span.style));
            } else {
                used += pw;
                lines
                    .last_mut()
                    .expect("never empty")
                    .push(Span::styled(piece, span.style));
            }
        }
    }
    lines
}

/// `label  value`, with the value wrapped under itself when it is long.
pub fn kv_wrapped(
    t: &Theme,
    label: &str,
    label_w: usize,
    value: Vec<Span<'static>>,
    room: usize,
) -> Vec<Line<'static>> {
    wrap_spans(value, room)
        .into_iter()
        .enumerate()
        .map(|(i, spans)| kv(t, if i == 0 { label } else { "" }, label_w, spans))
        .collect()
}

// --------------------------------------------------------------- click map

/// Where a click lands. Filled while a frame is drawn, read on the next mouse
/// event; later entries sit on top of earlier ones.
#[derive(Default)]
pub struct Hits {
    items: Vec<(Rect, Action)>,
}

impl Hits {
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn add(&mut self, rect: Rect, action: Action) {
        if rect.width > 0 && rect.height > 0 {
            self.items.push((rect, action));
        }
    }

    pub fn at(&self, x: u16, y: u16) -> Option<Action> {
        self.items
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|(_, a)| a.clone())
    }
}

/// One clickable key hint: the key in the accent colour, then what it does.
pub struct Chip<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub action: Action,
}

/// The space between two key hints.
pub const CHIP_GAP: u16 = 3;

impl Chip<'_> {
    pub fn width(&self) -> u16 {
        (width(self.key) + 1 + width(self.label)) as u16
    }
}

/// The width of a row of key hints.
pub fn chips_width(items: &[Chip]) -> u16 {
    let gaps = items.len().saturating_sub(1) as u16 * CHIP_GAP;
    items.iter().map(Chip::width).sum::<u16>() + gaps
}

/// Draw key hints left to right on one row and make each clickable. Hints
/// that do not fit are left out.
pub fn chips(f: &mut Frame, area: Rect, items: &[Chip], hits: &mut Hits, t: &Theme) {
    let mut x = area.x;
    let end = area.x + area.width;
    for chip in items {
        let text_w = chip.width();
        if x + text_w > end {
            break;
        }
        let rect = Rect::new(x, area.y, text_w, 1);
        let line = Line::from(vec![
            Span::styled(chip.key.to_string(), t.s_accent()),
            Span::styled(format!(" {}", chip.label), t.s_faint()),
        ]);
        f.render_widget(Paragraph::new(line), rect);
        hits.add(rect, chip.action.clone());
        x += text_w + CHIP_GAP;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_bar_is_always_its_width_and_shows_small_usage() {
        let t = Theme::detect(true, "violet");
        for (pct, pace) in [
            (0.0, None),
            (1.0, None),
            (50.0, Some(30.0)),
            (100.0, Some(100.0)),
        ] {
            let spans = bar(&t, pct, 20, pace);
            assert_eq!(width(&text(&spans)), 20, "pct {pct}");
        }
        // 1% of 20 cells rounds to nothing, but must still show one cell.
        let spans = bar(&t, 1.0, 20, None);
        assert_eq!(width(&spans[0].content), 1);
        assert!(text(&bar(&t, 50.0, 20, Some(30.0))).contains(BAR_PACE));
    }

    #[test]
    fn styled_text_wraps_at_spaces_and_keeps_its_styles() {
        let t = Theme::detect(true, "violet");
        let spans = vec![
            Span::styled("good for 28d", t.s_text()),
            Span::styled(" · until Sat 14 Nov, then sign in again", t.s_dim()),
        ];
        let lines = wrap_spans(spans, 20);
        assert!(lines.len() >= 3, "{lines:?}");
        for line in &lines {
            assert!(width(&text(line)) <= 20, "{line:?}");
        }
        assert_eq!(lines[0][0].style, t.s_text());
        assert!(!text(&lines[1]).starts_with(' '));
    }

    #[test]
    fn the_topmost_hit_wins() {
        let mut hits = Hits::default();
        hits.add(Rect::new(0, 0, 10, 10), Action::Back);
        hits.add(Rect::new(2, 2, 3, 1), Action::RefreshAll);
        assert!(matches!(hits.at(3, 2), Some(Action::RefreshAll)));
        assert!(matches!(hits.at(0, 0), Some(Action::Back)));
        assert!(hits.at(20, 20).is_none());
    }

    #[test]
    fn sparkline_has_one_cell_per_value() {
        assert_eq!(spark(&[0.0, 50.0, 100.0]).chars().count(), 3);
        assert_eq!(spark(&[100.0]), "█");
    }
}
