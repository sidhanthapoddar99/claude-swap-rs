//! A column of boxes, one per agent, that scrolls as one: the layout the
//! Usage page and the list on the Accounts page share.

use std::ops::Range;

use ratatui::widgets::Borders;

use crate::harness::Harness;

/// One agent's box in the column the page scrolls: the accounts it holds,
/// the line it starts on and how many lines it takes, borders included.
#[derive(Debug, PartialEq)]
pub struct Group {
    pub harness: Harness,
    pub accounts: Range<usize>,
    pub y: usize,
    pub h: usize,
}

impl Group {
    /// The part of the box between the lines `top` and `end` of the page,
    /// and the sides to draw: a box cut by the edge of the page is drawn
    /// without that side. `None` when no line of it is in view.
    pub fn shown(&self, top: usize, end: usize) -> Option<(usize, usize, Borders)> {
        let (from, to) = (self.y.max(top), (self.y + self.h).min(end));
        if from >= to {
            return None;
        }
        let mut sides = Borders::LEFT | Borders::RIGHT;
        if from == self.y {
            sides |= Borders::TOP;
        }
        if to == self.y + self.h {
            sides |= Borders::BOTTOM;
        }
        Some((from, to, sides))
    }
}

/// Where everything sits when the whole page is laid out top to bottom.
pub struct Plan {
    pub groups: Vec<Group>,
    /// The first line of each account.
    pub at: Vec<usize>,
    pub total: usize,
}

/// Lay the accounts out in boxes, one per agent. `lens` is how many lines
/// each account takes, and `gap` the blank lines between two accounts.
pub fn plan(agents: &[Harness], lens: &[usize], gap: usize) -> Plan {
    let mut groups: Vec<Group> = Vec::new();
    let mut at = Vec::with_capacity(lens.len());
    let mut y = 0;
    for (i, (h, len)) in agents.iter().zip(lens).enumerate() {
        if groups.last().is_some_and(|g| g.harness == *h) {
            y += gap;
        } else {
            // The bottom border of the box before, then the top of this one.
            if let Some(g) = groups.last_mut() {
                y += 1;
                g.h = y - g.y;
            }
            groups.push(Group {
                harness: *h,
                accounts: i..i,
                y,
                h: 0,
            });
            y += 1;
        }
        if let Some(g) = groups.last_mut() {
            g.accounts.end = i + 1;
        }
        at.push(y);
        y += len;
    }
    if let Some(g) = groups.last_mut() {
        y += 1;
        g.h = y - g.y;
    }
    Plan {
        groups,
        at,
        total: y,
    }
}

/// The first line to show so that the selected account is whole on screen,
/// with the border of its box when it is the first or the last one in it.
pub fn scroll(plan: &Plan, lens: &[usize], sel: usize, view_h: usize) -> usize {
    if plan.total <= view_h {
        return 0;
    }
    let Some(g) = plan.groups.iter().find(|g| g.accounts.contains(&sel)) else {
        return 0;
    };
    let top = plan.at[sel] - usize::from(g.accounts.start == sel);
    let bottom = plan.at[sel] + lens[sel] + usize::from(g.accounts.end == sel + 1);
    bottom.saturating_sub(view_h).min(top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_boxed_by_agent_and_the_selected_one_stays_whole() {
        use Harness::{Claude, Codex, Grok};
        let agents = [Claude, Claude, Codex, Grok];
        let lens = [3, 2, 2, 2];
        let p = plan(&agents, &lens, 1);
        // Claude: border, 3 lines, a gap, 2 lines, border.
        assert_eq!(
            p.groups[0],
            Group {
                harness: Claude,
                accounts: 0..2,
                y: 0,
                h: 8
            }
        );
        assert_eq!(p.groups[1].accounts, 2..3);
        assert_eq!((p.groups[1].y, p.groups[1].h), (8, 4));
        assert_eq!((p.groups[2].y, p.groups[2].h), (12, 4));
        assert_eq!(p.at, [1, 5, 9, 13]);
        assert_eq!(p.total, 16);

        // Everything fits: nothing scrolls.
        assert_eq!(scroll(&p, &lens, 3, 16), 0);
        // Ten lines: the first accounts need no scrolling.
        assert_eq!(scroll(&p, &lens, 0, 10), 0);
        assert_eq!(scroll(&p, &lens, 1, 10), 0);
        // The last one is shown to the bottom border of its box.
        assert_eq!(scroll(&p, &lens, 3, 10), 6);
        // Codex is alone in its box, so both of its borders are in view.
        assert_eq!(scroll(&p, &lens, 2, 10), 2);

        // Scrolled six lines down, ten in view: the first box has its last
        // two lines left and no top; the next two are whole.
        let all = Borders::ALL;
        assert_eq!(
            p.groups[0].shown(6, 16),
            Some((6, 8, all.difference(Borders::TOP)))
        );
        assert_eq!(p.groups[1].shown(6, 16), Some((8, 12, all)));
        assert_eq!(p.groups[2].shown(6, 16), Some((12, 16, all)));
        // Cut at the bottom, and out of view altogether.
        assert_eq!(
            p.groups[1].shown(0, 10),
            Some((8, 10, all.difference(Borders::BOTTOM)))
        );
        assert_eq!(p.groups[2].shown(0, 10), None);
    }
}
