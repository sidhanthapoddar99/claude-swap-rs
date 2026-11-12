//! The frame around every page: header, side menu, footer and dialogs.

use chrono::Utc;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{Action, App, Focus, Modal, Page, Tone};
use super::pages;
use super::theme::{ARROW, DOT, RING};
use super::widgets::{self, card, fit, fit_right, width, Chip, Hits};
use crate::harness::Harness;
use crate::health::{self, Level};
use crate::model;
use crate::refresh;

const NAV_W: u16 = 25;
/// A gap is left in the menu before each of these pages. First come the
/// pages about accounts, then the ones about what runs and how it is set up,
/// then the app itself.
const NAV_SPLITS: [usize; 2] = [5, 8];
pub const MIN_W: u16 = 84;
pub const MIN_H: u16 = 22;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let t = app.theme;
    f.render_widget(Block::default().style(t.base()), area);
    // Take the click map out while drawing, so pages can read the app freely.
    let mut hits = std::mem::take(&mut app.hits);
    hits.clear();

    if area.width < MIN_W || area.height < MIN_H {
        pages::empty(
            f,
            area,
            app,
            "cswap needs a little more room",
            &[
                &format!("this window is {}×{}", area.width, area.height),
                &format!("make it at least {MIN_W}×{MIN_H}"),
                "",
                "Esc quits",
            ],
        );
        app.hits = hits;
        return;
    }

    let [head, body, foot] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);
    let [nav, main] =
        Layout::horizontal([Constraint::Length(NAV_W), Constraint::Min(0)]).areas(body);

    // What the Help page's card holds: the page less its title, its margin,
    // and the card's border and padding. The keys scroll by it.
    app.help_view = (
        main.width.saturating_sub(6) as usize,
        main.height.saturating_sub(4) as usize,
    );
    header(f, head, app);
    sidebar(f, nav, app, &mut hits);
    page(f, main, app, &mut hits);
    footer(f, foot, app, &mut hits);
    if app.modal.is_some() {
        // Dim the page, so the dialog reads as being in front of it.
        let faint = t.faint();
        let buf = f.buffer_mut();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_fg(faint);
                    cell.modifier = Modifier::empty();
                }
            }
        }
        modal(f, area, app, &mut hits);
    }
    app.hits = hits;
}

fn header(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let now = Utc::now();
    let row = Rect::new(area.x + 2, area.y, area.width.saturating_sub(4), 1);

    let mut left = vec![
        Span::styled("◆ ", t.s_accent()),
        Span::styled("cswap", t.s_bold()),
        Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), t.s_faint()),
    ];
    if let Some(v) = app.derived.newer.as_ref().filter(|_| row.width >= 100) {
        left.push(Span::styled(
            format!("  ·  {v} is out: cswap update"),
            t.s(t.info()),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(left)), row);

    // When the numbers were last fetched, and when they will be again.
    let last = app
        .reg
        .accounts
        .iter()
        .filter_map(|a| a.meta.refresh.last_success)
        .max();
    let next = app
        .reg
        .accounts
        .iter()
        .filter_map(|a| refresh::next_due(a, &app.settings))
        .min();
    let mut right = Vec::new();
    if !app.busy.is_empty() {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let i = (now.timestamp_millis() / 120) as usize % frames.len();
        right.push(Span::styled(
            format!("{} refreshing", frames[i]),
            t.s(t.info()),
        ));
    } else {
        match last {
            Some(at) => right.push(Span::styled(
                format!("updated {}", model::ago(at, now)),
                t.s_dim(),
            )),
            None => right.push(Span::styled("no usage fetched yet", t.s_faint())),
        }
        if let Some(at) = next {
            let text = if at <= now {
                "next: now".to_string()
            } else {
                format!(
                    "next in {}",
                    model::span(at - now + chrono::Duration::seconds(59))
                )
            };
            right.push(Span::styled(format!("  ·  {text}"), t.s_faint()));
        }
    }
    let clock = now.with_timezone(&chrono::Local);
    let fmt = if app.settings.clock_24h {
        "%a %-d %b  %H:%M"
    } else {
        "%a %-d %b  %-I:%M %p"
    };
    right.push(Span::styled(
        format!("     {}", clock.format(fmt)),
        t.s_text(),
    ));
    f.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        row,
    );
}

fn sidebar(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let focused = app.focus == Focus::Nav && app.modal.is_none();
    let inner = card(f, area, "", t, focused.then(|| t.accent()));
    let attention = health::attention(&app.derived.checks);
    let w = inner.width as usize;
    let legend_h = Harness::ALL.len() as u16 + 2;
    let roomy = inner.height > (Page::ALL.len() + NAV_SPLITS.len()) as u16 + legend_h;

    for (i, page) in Page::ALL.into_iter().enumerate() {
        // The gaps set the groups of pages apart, when there is room.
        let gaps = NAV_SPLITS.iter().filter(|s| roomy && i >= **s).count();
        let y = inner.y + (i + gaps) as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, 1);
        let current = page == app.page;
        let (bar, style) = if current {
            (
                Span::styled("▌", t.s_accent()),
                Style::default()
                    .fg(t.text())
                    .bg(t.raised())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (Span::raw(" "), t.s_dim())
        };
        // A count beside the pages that have something waiting.
        let badge = match page {
            Page::Health if attention > 0 => Some((attention.to_string(), t.warn())),
            Page::Resets => {
                let n = crate::view::grant_count(&app.reg, Utc::now());
                (n > 0).then(|| (n.to_string(), t.info()))
            }
            Page::Sessions => {
                let n = app.sessions.len();
                (n > 0).then(|| (n.to_string(), t.ok()))
            }
            Page::Sync => {
                let n: usize = app
                    .reg
                    .accounts
                    .iter()
                    .map(|a| a.meta.sync.shadowed.len())
                    .sum();
                (n > 0).then(|| (n.to_string(), t.warn()))
            }
            _ => None,
        };
        let key = page.hotkey().to_string();
        let tail_w = badge.as_ref().map(|(b, _)| width(b) + 1).unwrap_or(0) + 2;
        let label = fit(page.title(), w.saturating_sub(2 + tail_w));
        let mut spans = vec![bar, Span::styled(format!(" {label}"), style)];
        if let Some((text, color)) = badge {
            spans.push(Span::styled(
                format!("{text} "),
                Style::default().fg(color).bg(style.bg.unwrap_or(t.panel())),
            ));
        }
        spans.push(Span::styled(
            fit_right(&key, 2),
            Style::default()
                .fg(t.faint())
                .bg(style.bg.unwrap_or(t.panel())),
        ));
        if current {
            pages::paint_row(f, rect, Style::default().bg(t.raised()));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), rect);
        hits.add(rect, Action::Go(page));
    }

    // The agents, with how many accounts each has.
    if inner.height > Page::ALL.len() as u16 + legend_h {
        let mut ly = inner.y + inner.height - legend_h;
        f.render_widget(
            Paragraph::new(Span::styled(
                " AGENTS",
                Style::default().fg(t.faint()).add_modifier(Modifier::BOLD),
            )),
            Rect::new(inner.x, ly, inner.width, 1),
        );
        ly += 1;
        for h in Harness::ALL {
            let count = app.reg.of(h).count();
            let installed = app.derived.bins.get(&h).is_some_and(|b| b.is_some());
            let (glyph, color) = if installed {
                (DOT, t.harness(h))
            } else {
                (RING, t.faint())
            };
            let note = if !installed {
                "–".to_string()
            } else {
                count.to_string()
            };
            let line = Line::from(vec![
                Span::styled(format!(" {glyph} "), Style::default().fg(color)),
                Span::styled(
                    fit(h.name(), w.saturating_sub(4 + width(&note))),
                    if installed { t.s_dim() } else { t.s_faint() },
                ),
                Span::styled(format!("{note} "), t.s_faint()),
            ]);
            f.render_widget(Paragraph::new(line), Rect::new(inner.x, ly, inner.width, 1));
            ly += 1;
        }
    }
}

fn page(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let [title, content] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    let line = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            app.page.title(),
            Style::default().fg(t.text()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "   {}",
                widgets::truncate(
                    app.page.blurb(),
                    (area.width as usize).saturating_sub(width(app.page.title()) + 6)
                )
            ),
            t.s_faint(),
        ),
    ]);
    f.render_widget(Paragraph::new(vec![Line::default(), line]), title);
    let content = Rect::new(
        content.x + 1,
        content.y,
        content.width.saturating_sub(2),
        content.height,
    );
    match app.page {
        Page::Overview => pages::overview::draw(f, content, app, hits),
        Page::Sync => pages::sync::draw(f, content, app, hits),
        Page::Usage => pages::usage::draw(f, content, app, hits),
        Page::Accounts => pages::accounts::draw(f, content, app, hits),
        Page::Root => pages::root::draw(f, content, app, hits),
        Page::Sessions => pages::sessions::draw(f, content, app, hits),
        Page::Resets => pages::resets::draw(f, content, app, hits),
        Page::Analytics => pages::analytics::draw(f, content, app, hits),
        Page::Health => pages::health::draw(f, content, app, hits),
        Page::Integrations => pages::integrations::draw(f, content, app, hits),
        Page::Settings => pages::settings::draw(f, content, app, hits),
        Page::Help => pages::help::draw(f, content, app),
    }
}

fn footer(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let [status, keys] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let pad = |r: Rect| Rect::new(r.x + 2, r.y, r.width.saturating_sub(4), 1);

    if let Some(toast) = &app.toast {
        let (glyph, color) = match toast.tone {
            Tone::Info => ("›", t.info()),
            Tone::Good => ("✓", t.ok()),
            Tone::Warn => ("▲", t.warn()),
            Tone::Bad => ("✗", t.bad()),
        };
        let text = widgets::truncate(&toast.text, pad(status).width.saturating_sub(3) as usize);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::styled(text, Style::default().fg(color)),
            ])),
            pad(status),
        );
    }

    let mut chips: Vec<Chip> = Vec::new();
    let key = app.selected_key();
    let on_account = matches!(
        app.page,
        Page::Overview | Page::Usage | Page::Accounts | Page::Analytics
    );
    if app.focus == Focus::Nav {
        chips.push(Chip {
            key: "↑↓",
            label: "pages",
            action: Action::Go(app.page),
        });
        chips.push(Chip {
            key: "⏎",
            label: "enter page",
            action: Action::Go(app.page),
        });
    } else if on_account {
        if let Some(k) = &key {
            chips.push(Chip {
                key: "⏎",
                label: "actions",
                action: Action::AccountMenu(k.clone()),
            });
            chips.push(Chip {
                key: "o",
                label: "open",
                action: Action::Launch(k.clone()),
            });
            chips.push(Chip {
                key: "d",
                label: "set default",
                action: Action::SetDefault(k.clone()),
            });
            if app.page == Page::Accounts {
                chips.push(Chip {
                    key: "l",
                    label: "sign in",
                    action: Action::Login(k.clone()),
                });
                chips.push(Chip {
                    key: "e",
                    label: "rename",
                    action: Action::EditLabel(k.clone()),
                });
                chips.push(Chip {
                    key: "b",
                    label: "renewal",
                    action: Action::EditBilling(k.clone()),
                });
                chips.push(Chip {
                    key: "x",
                    label: "remove",
                    action: Action::Remove(k.clone()),
                });
            }
        }
    } else if app.page == Page::Root {
        if let Some(k) = &key {
            chips.push(Chip {
                key: "⏎",
                label: "actions",
                action: Action::AccountMenu(k.clone()),
            });
            chips.push(Chip {
                key: "c",
                label: "make account",
                action: Action::NewFromRoot(k.clone()),
            });
            chips.push(Chip {
                key: "o",
                label: "open",
                action: Action::Launch(k.clone()),
            });
            chips.push(Chip {
                key: "d",
                label: "set default",
                action: Action::SetDefault(k.clone()),
            });
        }
    } else if app.page == Page::Sync {
        chips.push(Chip {
            key: "s",
            label: "sync all",
            action: Action::SyncAll,
        });
        if let Some(k) = &key {
            chips.push(Chip {
                key: "⏎",
                label: "sync this",
                action: Action::SyncOne(k.clone()),
            });
            chips.push(Chip {
                key: "R",
                label: "repair",
                action: Action::Repair(k.clone()),
            });
        }
    } else if app.page == Page::Settings {
        chips.push(Chip {
            key: "t",
            label: "next tab",
            action: Action::SettingsTab((app.settings_tab + 1) % pages::settings::TABS.len()),
        });
        chips.push(Chip {
            key: "⏎ ←→",
            label: "change",
            action: Action::SettingChange(0),
        });
    } else if app.page == Page::Health {
        chips.push(Chip {
            key: "⏎",
            label: "details",
            action: Action::Activate,
        });
    } else if app.page == Page::Resets {
        let row = pages::resets::rows(app)
            .get(app.row())
            .map(|r| r.key.clone());
        if row.is_some() {
            chips.push(Chip {
                key: "⏎",
                label: if row == app.spot {
                    "show all"
                } else {
                    "only this account"
                },
                action: Action::Activate,
            });
        }
    } else if app.page == Page::Integrations {
        chips.push(Chip {
            key: "⏎",
            label: "install · show",
            action: Action::Activate,
        });
    }
    chips.push(Chip {
        key: "a",
        label: "add account",
        action: Action::AddAccount,
    });
    chips.push(Chip {
        key: "r",
        label: "refresh",
        action: Action::RefreshAll,
    });
    // Help and quit keep their place at the right edge; the page's own keys
    // take what is left, most important first.
    let fixed = [
        Chip {
            key: "?",
            label: if app.page == Page::Help {
                "back"
            } else {
                "help"
            },
            action: Action::Help,
        },
        Chip {
            key: "esc",
            label: if app.focus == Focus::Nav {
                "quit"
            } else if app.spot.is_some() {
                "show all"
            } else {
                "menu"
            },
            action: Action::Back,
        },
    ];
    let fixed_w = widgets::chips_width(&fixed);
    let row = pad(keys);
    if row.width > fixed_w + 4 {
        let right = Rect::new(row.x + row.width - fixed_w, row.y, fixed_w, 1);
        let left = Rect::new(row.x, row.y, row.width - fixed_w - widgets::CHIP_GAP, 1);
        widgets::chips(f, right, &fixed, hits, t);
        widgets::chips(f, left, &chips, hits, t);
    } else {
        widgets::chips(f, row, &chips, hits, t);
    }
}

/// A dialog box centred on the screen.
fn dialog(f: &mut Frame, screen: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(screen.width.saturating_sub(4));
    let h = h.min(screen.height.saturating_sub(2));
    let rect = Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, rect);
    rect
}

fn modal(f: &mut Frame, screen: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let Some(modal) = &app.modal else { return };
    // Clicks that miss the dialog fall through to nothing, and close it.
    hits.clear();
    match modal {
        Modal::Choice {
            title,
            hint,
            items,
            sel,
        } => {
            let label_w = items.iter().map(|i| width(&i.label)).max().unwrap_or(10) + 2;
            let detail_w = items.iter().map(|i| width(&i.detail)).max().unwrap_or(0);
            let w = (label_w + detail_w + 12)
                .max(width(hint) + 6)
                .max(width(title) + 8)
                .max(44) as u16;
            // The groups get their titles and a blank line between them. On
            // a screen too short for that the blank lines go first, then the
            // titles, so every item is still on screen.
            let titled = items
                .iter()
                .filter(|i| i.heading.is_some_and(|h| !h.is_empty()))
                .count();
            let gaps = items.iter().skip(1).filter(|i| i.heading.is_some()).count();
            let chrome = if hint.is_empty() { 4 } else { 6 };
            let room = screen.height.saturating_sub(2) as usize;
            let titles = items.len() + titled + chrome <= room;
            let spaced = titles && items.len() + titled + gaps + chrome <= room;
            let h = items.len()
                + if titles { titled } else { 0 }
                + if spaced { gaps } else { 0 }
                + chrome;
            let rect = dialog(f, screen, w, h as u16);
            hits.add(
                rect,
                Action::ModalPick(*sel.min(&items.len().saturating_sub(1))),
            );
            let inner = card(f, rect, title, t, Some(t.accent()));
            hits.clear();
            let mut y = inner.y;
            if !hint.is_empty() {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        widgets::truncate(hint, inner.width as usize),
                        t.s_faint(),
                    )),
                    Rect::new(inner.x, y, inner.width, 1),
                );
                y += 2;
            } else {
                y += 1;
            }
            for (i, item) in items.iter().enumerate() {
                if let Some(heading) = item.heading {
                    if spaced && i > 0 {
                        y += 1;
                    }
                    if titles && !heading.is_empty() && y < inner.y + inner.height {
                        f.render_widget(
                            Paragraph::new(Span::styled(
                                format!("  {heading}"),
                                Style::default().fg(t.faint()).add_modifier(Modifier::BOLD),
                            )),
                            Rect::new(inner.x, y, inner.width, 1),
                        );
                        y += 1;
                    }
                }
                if y >= inner.y + inner.height {
                    break;
                }
                let row = Rect::new(inner.x, y, inner.width, 1);
                let selected = i == *sel;
                let bg = if selected { t.raised() } else { t.panel() };
                pages::paint_row(f, row, Style::default().bg(bg));
                let fg = if !item.enabled {
                    t.faint()
                } else if selected {
                    t.text()
                } else {
                    t.dim()
                };
                let key = item.key.map(|c| c.to_string()).unwrap_or_default();
                let mut spans = vec![
                    Span::styled(
                        if selected {
                            format!("{ARROW} ")
                        } else {
                            "  ".to_string()
                        },
                        Style::default().fg(t.accent()).bg(bg),
                    ),
                    Span::styled(
                        fit(&key, 2),
                        Style::default()
                            .fg(if item.enabled { t.accent() } else { t.faint() })
                            .bg(bg),
                    ),
                    Span::styled(
                        fit(&item.label, label_w),
                        Style::default().fg(fg).bg(bg).add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                    ),
                ];
                let room = (inner.width as usize).saturating_sub(label_w + 5);
                spans.push(Span::styled(
                    widgets::truncate(&item.detail, room),
                    Style::default().fg(t.faint()).bg(bg),
                ));
                f.render_widget(Paragraph::new(Line::from(spans)), row);
                hits.add(row, Action::ModalPick(i));
                y += 1;
            }
            let foot = Rect::new(inner.x, rect.y + rect.height - 2, inner.width, 1);
            widgets::chips(
                f,
                foot,
                &[
                    Chip {
                        key: "⏎",
                        label: "choose",
                        action: Action::ModalOk,
                    },
                    Chip {
                        key: "esc",
                        label: "cancel",
                        action: Action::ModalCancel,
                    },
                ],
                hits,
                t,
            );
        }
        Modal::Ticks {
            title,
            hint,
            items,
            sel,
            ..
        } => {
            let label_w = items.iter().map(|i| width(&i.label)).max().unwrap_or(8) + 2;
            let detail_w = items
                .iter()
                .map(|i| width(&i.detail) + 12)
                .max()
                .unwrap_or(0);
            let w = (label_w + detail_w + 10)
                .max(width(hint) + 6)
                .max(width(title) + 8)
                .max(48) as u16;
            let rect = dialog(f, screen, w, items.len() as u16 + 6);
            let inner = card(f, rect, title, t, Some(t.accent()));
            // A click on the dialog that misses every box must not close it:
            // the ticks made so far would be lost.
            hits.add(rect, Action::Ignore);
            f.render_widget(
                Paragraph::new(Span::styled(
                    widgets::truncate(hint, inner.width as usize),
                    t.s_faint(),
                )),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            for (i, item) in items.iter().enumerate() {
                let y = inner.y + 2 + i as u16;
                if y >= inner.y + inner.height {
                    break;
                }
                let row = Rect::new(inner.x, y, inner.width, 1);
                let selected = i == *sel;
                let bg = if selected { t.raised() } else { t.panel() };
                pages::paint_row(f, row, Style::default().bg(bg));
                // Green is what is saved, yellow what this dialog changed.
                let changed = item.on != item.was;
                let color = if !item.enabled {
                    t.faint()
                } else if changed {
                    t.warn()
                } else if item.on {
                    t.ok()
                } else {
                    t.dim()
                };
                let note = match (changed, item.on) {
                    _ if !item.enabled => "",
                    (true, true) => " · not saved",
                    (true, false) => " · unticked, not saved",
                    _ => "",
                };
                let room = (inner.width as usize).saturating_sub(label_w + 6);
                let spans = vec![
                    Span::styled(
                        if selected {
                            format!("{ARROW} ")
                        } else {
                            "  ".to_string()
                        },
                        Style::default().fg(t.accent()).bg(bg),
                    ),
                    Span::styled(
                        if item.on { "[x] " } else { "[ ] " },
                        Style::default().fg(color).bg(bg),
                    ),
                    Span::styled(
                        fit(&item.label, label_w),
                        Style::default()
                            .fg(if item.enabled && (item.on || changed) {
                                color
                            } else if item.enabled {
                                t.text()
                            } else {
                                t.faint()
                            })
                            .bg(bg)
                            .add_modifier(if selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                    Span::styled(
                        widgets::truncate(&format!("{}{note}", item.detail), room),
                        Style::default()
                            .fg(if changed { t.warn() } else { t.faint() })
                            .bg(bg),
                    ),
                ];
                f.render_widget(Paragraph::new(Line::from(spans)), row);
                hits.add(row, Action::Tick(i));
            }
            widgets::chips(
                f,
                Rect::new(inner.x, rect.y + rect.height - 2, inner.width, 1),
                &[
                    Chip {
                        key: "space",
                        label: "tick",
                        action: Action::Tick(*sel),
                    },
                    Chip {
                        key: "⏎",
                        label: "save",
                        action: Action::ModalOk,
                    },
                    Chip {
                        key: "esc",
                        label: "cancel",
                        action: Action::ModalCancel,
                    },
                ],
                hits,
                t,
            );
        }
        Modal::Input {
            title,
            prompt,
            value,
            hint,
            error,
            ..
        } => {
            let w = (width(hint) + 6).clamp(56, 96) as u16;
            let rect = dialog(f, screen, w, 9);
            let inner = card(f, rect, title, t, Some(t.accent()));
            let line = |y: u16| Rect::new(inner.x, inner.y + y, inner.width, 1);
            f.render_widget(
                Paragraph::new(Span::styled(prompt.clone(), t.s_dim())),
                line(1),
            );
            // The field: the tail of the value when it is longer than the box.
            let field_w = inner.width.saturating_sub(2) as usize;
            let shown: String = {
                let chars: Vec<char> = value.chars().collect();
                let start = chars.len().saturating_sub(field_w.saturating_sub(1));
                chars[start..].iter().collect()
            };
            let field = Line::from(vec![
                Span::styled(
                    format!(" {shown}"),
                    Style::default().fg(t.text()).bg(t.raised()),
                ),
                Span::styled("▏", Style::default().fg(t.accent()).bg(t.raised())),
                Span::styled(
                    " ".repeat(field_w.saturating_sub(width(&shown) + 1)),
                    Style::default().bg(t.raised()),
                ),
            ]);
            f.render_widget(Paragraph::new(field), line(2));
            let note = match error {
                Some(e) => Span::styled(e.clone(), t.s(t.bad())),
                None => Span::styled(hint.clone(), t.s_faint()),
            };
            f.render_widget(
                Paragraph::new(Line::from(note)).wrap(Wrap { trim: true }),
                Rect::new(inner.x, inner.y + 4, inner.width, 2),
            );
            widgets::chips(
                f,
                Rect::new(inner.x, rect.y + rect.height - 2, inner.width, 1),
                &[
                    Chip {
                        key: "⏎",
                        label: "ok",
                        action: Action::ModalOk,
                    },
                    Chip {
                        key: "esc",
                        label: "cancel",
                        action: Action::ModalCancel,
                    },
                ],
                hits,
                t,
            );
        }
        Modal::Confirm {
            title,
            body,
            yes,
            danger,
            ..
        } => {
            let w = body.iter().map(|l| width(l)).max().unwrap_or(30).max(40) as u16 + 6;
            let rect = dialog(f, screen, w, body.len() as u16 + 6);
            let accent = if *danger { t.bad() } else { t.accent() };
            let inner = card(f, rect, title, t, Some(accent));
            let lines: Vec<Line> = std::iter::once(Line::default())
                .chain(
                    body.iter()
                        .map(|l| Line::from(Span::styled(l.clone(), t.s_text()))),
                )
                .collect();
            f.render_widget(Paragraph::new(lines), inner);
            let key = if *danger { "y" } else { "⏎" };
            widgets::chips(
                f,
                Rect::new(inner.x, rect.y + rect.height - 2, inner.width, 1),
                &[
                    Chip {
                        key,
                        label: yes,
                        action: Action::ModalOk,
                    },
                    Chip {
                        key: "esc",
                        label: "cancel",
                        action: Action::ModalCancel,
                    },
                ],
                hits,
                t,
            );
        }
        Modal::Info { title, body } => {
            let w = body
                .iter()
                .map(|l| width(l))
                .max()
                .unwrap_or(30)
                .clamp(40, 100) as u16
                + 6;
            let rect = dialog(f, screen, w, body.len() as u16 + 5);
            hits.add(rect, Action::ModalCancel);
            let inner = card(f, rect, title, t, Some(t.accent()));
            let lines: Vec<Line> = std::iter::once(Line::default())
                .chain(body.iter().map(|l| {
                    // A line with no indent is a section heading.
                    let heading = !l.starts_with(' ') && !l.is_empty() && body.len() > 6;
                    Line::from(Span::styled(
                        l.clone(),
                        if heading {
                            t.s_faint().add_modifier(Modifier::BOLD)
                        } else {
                            t.s_text()
                        },
                    ))
                }))
                .collect();
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
            f.render_widget(
                Paragraph::new(Span::styled("any key closes", t.s_faint())),
                Rect::new(inner.x, rect.y + rect.height - 2, inner.width, 1),
            );
        }
    }
}

/// The level of a health check as a glyph and a colour.
pub fn level_mark(app: &App, level: Level) -> Span<'static> {
    let t = &app.theme;
    let (glyph, color) = match level {
        Level::Ok => (super::theme::CHECK, t.ok()),
        Level::Info => ("·", t.dim()),
        Level::Warn => (super::theme::WARN, t.warn()),
        Level::Bad => (super::theme::CROSS, t.bad()),
    };
    Span::styled(glyph.to_string(), Style::default().fg(color))
}
