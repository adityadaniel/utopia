//! The pane stack (DESIGN.md §4.3, §4.4; PRD §9.2.1 SP-1…SP-9): one list of open pages,
//! Daily Notes always first. Panes mode shows every entry side by side; Single mode shows
//! the last one. Switching modes never changes the list (SP-6).
//!
//! Everything here is pure: no gpui, no store. The workspace owns one page view per entry
//! and applies a [`Plan`] to that list, so the click rules and the sticky geometry can be
//! read, changed and tested in one place.

use utopia_core::{BlockId, PageId};

/// What one entry of the stack shows. `Daily` is the Daily Notes feed, always entry 0;
/// a single day opened on its own is a `Page` like any other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Daily,
    Page { page: PageId, zoom: Option<BlockId> },
}

impl Entry {
    pub fn page(page: PageId) -> Self {
        Entry::Page { page, zoom: None }
    }

    /// One line of the saved stack (`local_state`, rule 9): `daily`, `page <id>` or
    /// `page <id> <zoom>`. Ids are ULIDs or UUIDs, so they never hold a space.
    pub fn encode(&self) -> String {
        match self {
            Entry::Daily => "daily".into(),
            Entry::Page { page, zoom: None } => format!("page {}", page.as_str()),
            Entry::Page {
                page,
                zoom: Some(z),
            } => format!("page {} {}", page.as_str(), z.0),
        }
    }

    /// The inverse of [`Entry::encode`]; a line it cannot read is `None`, never a panic
    /// (rule 11): a stack written by a later version just loses that entry.
    pub fn decode(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        match (parts.next()?, parts.next(), parts.next(), parts.next()) {
            ("daily", None, None, None) => Some(Entry::Daily),
            ("page", Some(p), zoom, None) => Some(Entry::Page {
                page: PageId(p.to_string()),
                zoom: zoom.map(|z| BlockId(z.to_string())),
            }),
            _ => None,
        }
    }
}

/// The view mode (DESIGN.md §4.4), an app setting in `settings.json` (rule 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    Panes,
    /// The default (SP-6): one page at a time, like the app before stacked panes.
    #[default]
    Single,
}

impl ViewMode {
    pub fn key(self) -> &'static str {
        match self {
            ViewMode::Panes => "panes",
            ViewMode::Single => "single",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "panes" => Some(ViewMode::Panes),
            "single" => Some(ViewMode::Single),
            _ => None,
        }
    }
}

/// How a link was clicked. `Secondary` is ⌘ on macOS and Ctrl elsewhere
/// (`gpui::Modifiers::secondary`); ⌘⇧ counts as `Secondary` too (SP-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Plain,
    Secondary,
}

/// Where the click came from: a link inside the pane at this index, or anything outside
/// the stack (the left sidebar, Quick jump, the calendar, the right sidebar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Pane(usize),
    Outside,
}

/// The new stack, said in terms of the old one: the old entries at `keep`, in that order,
/// then `push` if there is one. The workspace keeps each kept entry's page view as it is
/// (scroll position, folds), so adding or closing a pane never reloads the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub keep: Vec<usize>,
    pub push: Option<Entry>,
    pub mode: ViewMode,
    /// The entry of the *new* stack to focus and bring into view.
    pub focus: usize,
}

impl Plan {
    /// The stack `self` turns `old` into.
    pub fn apply<T: Clone>(&self, old: &[T], new: impl FnOnce(&Entry) -> T) -> Vec<T> {
        let mut out: Vec<T> = self
            .keep
            .iter()
            .filter_map(|&i| old.get(i).cloned())
            .collect();
        if let Some(e) = &self.push {
            out.push(new(e));
        }
        out
    }

    /// Whether the plan leaves the list as it was (only focus or scroll may move).
    pub fn is_noop(&self, len: usize) -> bool {
        self.push.is_none()
            && self.keep.len() == len
            && self.keep.iter().enumerate().all(|(a, &b)| a == b)
    }

    fn truncate(keep: usize, push: Option<Entry>, mode: ViewMode) -> Self {
        let focus = if push.is_some() {
            keep
        } else {
            keep.saturating_sub(1)
        };
        Plan {
            keep: (0..keep).collect(),
            push,
            mode,
            focus,
        }
    }

    fn stay(len: usize, mode: ViewMode, focus: usize) -> Self {
        Plan {
            keep: (0..len).collect(),
            push: None,
            mode,
            focus,
        }
    }
}

/// Open `target` (SP-2…SP-5). The click rules, in one place:
///
/// | mode   | click     | from      | target already at `j`   | otherwise                 |
/// |--------|-----------|-----------|-------------------------|---------------------------|
/// | Panes  | plain     | pane `i`  | scroll to `j`           | keep `0..=i`, add         |
/// | Panes  | secondary | pane `i>0`| scroll to `j`           | keep `0..i`, add (replace)|
/// | Panes  | secondary | pane 0    | as plain                | as plain                  |
/// | Panes  | any       | outside   | scroll to `j`           | add at the end            |
/// | Single | plain     | anywhere  | cut back to `j`         | add at the end            |
/// | Single | secondary | anywhere  | Panes, scroll to `j`    | Panes, add at the end     |
pub fn open(stack: &[Entry], mode: ViewMode, origin: Origin, click: Click, target: Entry) -> Plan {
    let len = stack.len();
    let found = stack.iter().position(|e| *e == target);
    match mode {
        ViewMode::Panes => {
            if let Some(j) = found {
                return Plan::stay(len, mode, j);
            }
            match origin {
                Origin::Outside => Plan::truncate(len, Some(target), mode),
                Origin::Pane(i) => {
                    let i = i.min(len.saturating_sub(1));
                    let keep = match click {
                        // The Daily Notes pane is never replaced (§4.3).
                        Click::Secondary if i > 0 => i,
                        _ => i + 1,
                    };
                    Plan::truncate(keep, Some(target), mode)
                }
            }
        }
        ViewMode::Single => match (click, found) {
            (Click::Plain, Some(j)) => Plan::truncate(j + 1, None, mode),
            (Click::Plain, None) => Plan::truncate(len, Some(target), mode),
            // ⌘-click peeks the page beside the current one (§4.4).
            (Click::Secondary, Some(j)) => Plan::stay(len, ViewMode::Panes, j),
            (Click::Secondary, None) => Plan::truncate(len, Some(target), ViewMode::Panes),
        },
    }
}

/// Close the entry at `i` (the × or ⌘W, SP-4). Daily Notes (entry 0) never closes.
pub fn close(len: usize, mode: ViewMode, i: usize) -> Plan {
    if i == 0 || i >= len {
        return Plan::stay(len, mode, i.min(len.saturating_sub(1)));
    }
    Plan {
        keep: (0..len).filter(|&k| k != i).collect(),
        push: None,
        mode,
        // The pane that slid into the closed one's place, or the new last one.
        focus: i.min(len - 2),
    }
}

/// Back (SP-5): remove the last entry, in both modes.
pub fn back(len: usize, mode: ViewMode) -> Plan {
    close(len, mode, len.saturating_sub(1))
}

/// "Close other panes" (SP-4): keep Daily Notes and the focused pane.
pub fn close_others(len: usize, mode: ViewMode, focused: usize) -> Plan {
    let keep: Vec<usize> = if focused == 0 || focused >= len {
        vec![0]
    } else {
        vec![0, focused]
    };
    let focus = keep.len() - 1;
    Plan {
        keep,
        push: None,
        mode,
        focus,
    }
}

/// "Daily Notes" in the sidebar (SP-7): the stack is Daily Notes alone.
pub fn reset(mode: ViewMode) -> Plan {
    Plan::truncate(1, None, mode)
}

/// A title in the top bar's trail (SP-5): Panes scrolls to it, Single cuts back to it.
pub fn trail(len: usize, mode: ViewMode, j: usize) -> Plan {
    let j = j.min(len.saturating_sub(1));
    match mode {
        ViewMode::Panes => Plan::stay(len, mode, j),
        ViewMode::Single => Plan::truncate(j + 1, None, mode),
    }
}

/// Entry `i` became what another entry already shows, without a click: a rename merged
/// its page into one that is open (§8.6), or it zoomed (in or out) to what another entry
/// shows. One copy stays: entry `i`, the one in use, with its caret, scroll and focus; the
/// other copy goes. `None` when `i` has no twin.
pub fn dedupe(stack: &[Entry], mode: ViewMode, i: usize) -> Option<Plan> {
    if i == 0 {
        return None;
    }
    let target = stack.get(i)?;
    let j = stack
        .iter()
        .enumerate()
        .position(|(k, e)| k != i && e == target)?;
    if j == 0 {
        return None;
    }
    let mut plan = close(stack.len(), mode, j);
    plan.focus = if i > j { i - 1 } else { i };
    Some(plan)
}

/// The mode toggle (SP-6): the list stays; Panes scrolls to the end, Single shows the last.
pub fn set_mode(len: usize, mode: ViewMode) -> Plan {
    Plan::stay(len, mode, len.saturating_sub(1))
}

/// The stack read back at launch (SP-8): Daily Notes first whatever was saved, then the
/// saved entries that still `exist`, each once, in their order.
pub fn restore(saved: &str, exists: impl Fn(&Entry) -> bool) -> Vec<Entry> {
    let mut out = vec![Entry::Daily];
    for e in saved.lines().filter_map(Entry::decode) {
        if e != Entry::Daily && !out.contains(&e) && exists(&e) {
            out.push(e);
        }
    }
    out
}

/// The saved form of `stack`, one [`Entry::encode`] line per entry.
pub fn save(stack: &[Entry]) -> String {
    stack
        .iter()
        .map(Entry::encode)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The sticky geometry of Panes mode (§4.3), in points from the scroller's left edge.
///
/// CSS would lay the panes out in a row and stick pane `i` at `i × spine`. gpui has no
/// `position: sticky`, so the workspace places each pane at `left(i)` itself. One rule is
/// added: when the scroller is too narrow for every spine and a whole pane, the oldest
/// spines slide under one another at the left edge (`shift`) instead of pushing the last
/// pane off the right side, so a deep stack in a small window still shows its newest pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub pane: f32,
    pub spine: f32,
    /// A pane showing less than this becomes a spine.
    pub threshold: f32,
    pub viewport: f32,
    pub count: usize,
}

impl Geometry {
    /// The geometry for a scroller `viewport` wide. A pane is `pane` wide, or narrower when
    /// the scroller is (both sidebars open in a small window): a pane wider than the
    /// scroller would put its right side, and its close button, out of reach. With more
    /// than one entry it also leaves room for one spine, so the older panes stay in sight
    /// and a click away.
    pub fn fit(pane: f32, spine: f32, threshold: f32, viewport: f32, count: usize) -> Self {
        let viewport = viewport.max(1.);
        let room = if count > 1 {
            viewport - spine
        } else {
            viewport
        };
        Geometry {
            pane: pane.min(room).max(spine.min(viewport)),
            spine,
            threshold,
            viewport,
            count,
        }
    }

    /// How far the row scrolls: the panes laid end to end, less what the scroller shows.
    pub fn max_scroll(&self) -> f32 {
        (self.count as f32 * self.pane - self.viewport).max(0.)
    }

    fn shift(&self) -> f32 {
        let stuck = self.count.saturating_sub(1) as f32 * self.spine + self.pane;
        (stuck - self.viewport).max(0.)
    }

    /// Where pane `i` sticks.
    pub fn stuck(&self, i: usize) -> f32 {
        (i as f32 * self.spine - self.shift()).max(0.)
    }

    /// Pane `i`'s left edge at `scroll`.
    pub fn left(&self, i: usize, scroll: f32) -> f32 {
        (i as f32 * self.pane - scroll).max(self.stuck(i))
    }

    /// How much of pane `i` shows before the next pane covers it.
    pub fn visible(&self, i: usize, scroll: f32) -> f32 {
        if i + 1 >= self.count {
            return self.pane;
        }
        (self.left(i + 1, scroll) - self.left(i, scroll)).clamp(0., self.pane)
    }

    /// Pane `i` is drawn as its spine (§4.3): less than `threshold` of it shows.
    pub fn is_spine(&self, i: usize, scroll: f32) -> bool {
        self.visible(i, scroll) < self.threshold
    }

    /// Pane `i` starts past the scroller's right edge: nothing of it is drawn.
    pub fn off_screen(&self, i: usize, scroll: f32) -> bool {
        self.left(i, scroll) >= self.viewport
    }

    /// The spine click (§4.3): the scroll that stands pane `i` whole at its sticky place,
    /// `index × paneWidth − stuck(i)`. With no folded spines `stuck(i)` is `index × spine`,
    /// so this is the spec's `index × (paneWidth − spine)` exactly.
    pub fn spine_target(&self, i: usize) -> f32 {
        (i as f32 * self.pane - self.stuck(i)).clamp(0., self.max_scroll())
    }

    /// The nearest scroll to `scroll` at which pane `i` shows whole: not covered by the
    /// next pane and not past the right edge (SP-9, focus moves and opens).
    pub fn reveal(&self, i: usize, scroll: f32) -> f32 {
        let lo = (i + 1) as f32 * self.pane - self.viewport;
        let hi = i as f32 * self.pane - self.stuck(i);
        scroll.clamp(lo.min(hi), hi).clamp(0., self.max_scroll())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str) -> Entry {
        Entry::page(PageId(id.into()))
    }

    /// Daily Notes, then pages a, b, c.
    fn abc() -> Vec<Entry> {
        vec![Entry::Daily, p("a"), p("b"), p("c")]
    }

    fn after(stack: &[Entry], plan: &Plan) -> Vec<Entry> {
        plan.apply(stack, Entry::clone)
    }

    // ---- Panes mode (SP-2, SP-3) ----

    #[test]
    fn sp_2_plain_click_drops_the_right_and_adds_after_the_pane() {
        let s = abc();
        let plan = open(&s, ViewMode::Panes, Origin::Pane(1), Click::Plain, p("x"));
        assert_eq!(after(&s, &plan), vec![Entry::Daily, p("a"), p("x")]);
        assert_eq!(plan.focus, 2);
        assert_eq!(plan.mode, ViewMode::Panes);
    }

    #[test]
    fn sp_2_plain_click_in_the_last_pane_appends() {
        let s = abc();
        let plan = open(&s, ViewMode::Panes, Origin::Pane(3), Click::Plain, p("x"));
        assert_eq!(
            after(&s, &plan),
            vec![Entry::Daily, p("a"), p("b"), p("c"), p("x")]
        );
        assert_eq!(plan.focus, 4);
    }

    #[test]
    fn sp_3_secondary_click_replaces_the_pane_in_place() {
        let s = abc();
        let plan = open(
            &s,
            ViewMode::Panes,
            Origin::Pane(2),
            Click::Secondary,
            p("x"),
        );
        assert_eq!(after(&s, &plan), vec![Entry::Daily, p("a"), p("x")]);
        assert_eq!(plan.focus, 2);
    }

    #[test]
    fn sp_3_secondary_click_never_replaces_daily_notes() {
        let s = abc();
        let plain = open(&s, ViewMode::Panes, Origin::Pane(0), Click::Plain, p("x"));
        let cmd = open(
            &s,
            ViewMode::Panes,
            Origin::Pane(0),
            Click::Secondary,
            p("x"),
        );
        assert_eq!(after(&s, &cmd), vec![Entry::Daily, p("x")]);
        assert_eq!(cmd, plain);
    }

    #[test]
    fn sp_2_an_open_target_is_scrolled_to_not_added() {
        let s = abc();
        for click in [Click::Plain, Click::Secondary] {
            for origin in [Origin::Pane(3), Origin::Pane(0), Origin::Outside] {
                let plan = open(&s, ViewMode::Panes, origin, click, p("a"));
                assert!(plan.is_noop(s.len()), "{click:?} from {origin:?}");
                assert_eq!(plan.focus, 1);
            }
        }
        // Daily Notes is a target like any other.
        let plan = open(
            &s,
            ViewMode::Panes,
            Origin::Outside,
            Click::Plain,
            Entry::Daily,
        );
        assert!(plan.is_noop(s.len()));
        assert_eq!(plan.focus, 0);
    }

    #[test]
    fn sp_7_outside_opens_at_the_end() {
        let s = abc();
        let plan = open(&s, ViewMode::Panes, Origin::Outside, Click::Plain, p("x"));
        assert_eq!(
            after(&s, &plan),
            vec![Entry::Daily, p("a"), p("b"), p("c"), p("x")]
        );
        let single = open(&s, ViewMode::Single, Origin::Outside, Click::Plain, p("x"));
        assert_eq!(after(&s, &single), after(&s, &plan));
    }

    #[test]
    fn a_zoomed_page_is_its_own_entry() {
        let zoomed = Entry::Page {
            page: PageId("a".into()),
            zoom: Some(BlockId("b1".into())),
        };
        let s = abc();
        let plan = open(
            &s,
            ViewMode::Panes,
            Origin::Pane(3),
            Click::Plain,
            zoomed.clone(),
        );
        assert_eq!(after(&s, &plan).last(), Some(&zoomed));
    }

    // ---- Single mode (SP-5, SP-6) ----

    #[test]
    fn sp_5_single_plain_click_navigates_forward() {
        let s = abc();
        let plan = open(&s, ViewMode::Single, Origin::Pane(3), Click::Plain, p("x"));
        assert_eq!(
            after(&s, &plan),
            vec![Entry::Daily, p("a"), p("b"), p("c"), p("x")]
        );
        assert_eq!(plan.focus, 4);
        assert_eq!(plan.mode, ViewMode::Single);
    }

    #[test]
    fn sp_5_single_plain_click_on_an_open_entry_cuts_back_to_it() {
        let s = abc();
        let plan = open(&s, ViewMode::Single, Origin::Pane(3), Click::Plain, p("a"));
        assert_eq!(after(&s, &plan), vec![Entry::Daily, p("a")]);
        assert_eq!(plan.focus, 1);
        let home = open(
            &s,
            ViewMode::Single,
            Origin::Outside,
            Click::Plain,
            Entry::Daily,
        );
        assert_eq!(after(&s, &home), vec![Entry::Daily]);
    }

    #[test]
    fn sp_6_single_secondary_click_peeks_in_panes_mode() {
        let s = abc();
        let plan = open(
            &s,
            ViewMode::Single,
            Origin::Pane(3),
            Click::Secondary,
            p("x"),
        );
        assert_eq!(plan.mode, ViewMode::Panes);
        assert_eq!(
            after(&s, &plan),
            vec![Entry::Daily, p("a"), p("b"), p("c"), p("x")]
        );
        let open_one = open(
            &s,
            ViewMode::Single,
            Origin::Pane(3),
            Click::Secondary,
            p("b"),
        );
        assert_eq!(open_one.mode, ViewMode::Panes);
        assert!(open_one.is_noop(s.len()));
        assert_eq!(open_one.focus, 2);
    }

    // ---- Close, back, reset, trail (SP-4, SP-5, SP-7) ----

    #[test]
    fn sp_4_close_removes_only_that_pane() {
        let s = abc();
        let plan = close(s.len(), ViewMode::Panes, 2);
        assert_eq!(after(&s, &plan), vec![Entry::Daily, p("a"), p("c")]);
        assert_eq!(plan.focus, 2, "the pane that slid left takes focus");
        let last = close(s.len(), ViewMode::Panes, 3);
        assert_eq!(last.focus, 2);
    }

    #[test]
    fn sp_4_daily_notes_never_closes() {
        let s = abc();
        assert!(close(s.len(), ViewMode::Panes, 0).is_noop(s.len()));
        assert!(close(s.len(), ViewMode::Panes, 9).is_noop(s.len()));
        assert!(
            back(1, ViewMode::Single).is_noop(1),
            "Daily Notes alone has no Back"
        );
    }

    #[test]
    fn sp_5_back_removes_the_last_entry() {
        let s = abc();
        for mode in [ViewMode::Panes, ViewMode::Single] {
            let plan = back(s.len(), mode);
            assert_eq!(after(&s, &plan), vec![Entry::Daily, p("a"), p("b")]);
            assert_eq!(plan.focus, 2);
        }
    }

    #[test]
    fn sp_4_close_others_keeps_daily_and_the_focused_pane() {
        let s = abc();
        let plan = close_others(s.len(), ViewMode::Panes, 2);
        assert_eq!(after(&s, &plan), vec![Entry::Daily, p("b")]);
        assert_eq!(plan.focus, 1);
        let from_daily = close_others(s.len(), ViewMode::Panes, 0);
        assert_eq!(after(&s, &from_daily), vec![Entry::Daily]);
    }

    #[test]
    fn sp_7_reset_leaves_daily_notes_alone() {
        let s = abc();
        for mode in [ViewMode::Panes, ViewMode::Single] {
            let plan = reset(mode);
            assert_eq!(after(&s, &plan), vec![Entry::Daily]);
            assert_eq!(plan.focus, 0);
            assert_eq!(plan.mode, mode);
        }
    }

    #[test]
    fn sp_5_trail_scrolls_in_panes_and_cuts_back_in_single() {
        let s = abc();
        let panes = trail(s.len(), ViewMode::Panes, 1);
        assert!(panes.is_noop(s.len()));
        assert_eq!(panes.focus, 1);
        let single = trail(s.len(), ViewMode::Single, 1);
        assert_eq!(after(&s, &single), vec![Entry::Daily, p("a")]);
    }

    // ---- Mode switch and restore (SP-6, SP-8) ----

    #[test]
    fn sp_6_switching_modes_keeps_the_list() {
        let s = abc();
        for mode in [ViewMode::Panes, ViewMode::Single] {
            let plan = set_mode(s.len(), mode);
            assert!(plan.is_noop(s.len()));
            assert_eq!(plan.mode, mode);
            assert_eq!(
                plan.focus, 3,
                "Panes scrolls to the end; Single shows the last"
            );
        }
    }

    #[test]
    fn sp_8_restore_round_trips_and_drops_what_is_gone() {
        let zoomed = Entry::Page {
            page: PageId("b".into()),
            zoom: Some(BlockId("01J".into())),
        };
        let s = vec![Entry::Daily, p("a"), zoomed.clone(), p("gone")];
        let text = save(&s);
        let back = restore(&text, |e| *e != p("gone"));
        assert_eq!(back, vec![Entry::Daily, p("a"), zoomed]);
    }

    #[test]
    fn sp_8_restore_always_starts_with_daily_notes_once() {
        assert_eq!(restore("", |_| true), vec![Entry::Daily]);
        let odd = "page a\ndaily\nnonsense line here x\npage a\n\npage b";
        assert_eq!(restore(odd, |_| true), vec![Entry::Daily, p("a"), p("b")]);
    }

    #[test]
    fn view_mode_keys_round_trip() {
        for m in [ViewMode::Panes, ViewMode::Single] {
            assert_eq!(ViewMode::from_key(m.key()), Some(m));
        }
        assert_eq!(ViewMode::from_key("tabs"), None);
        assert_eq!(ViewMode::default(), ViewMode::Single);
    }

    // ---- Geometry (§4.3) ----

    fn geo(count: usize, viewport: f32) -> Geometry {
        Geometry {
            pane: 560.,
            spine: 40.,
            threshold: 80.,
            viewport,
            count,
        }
    }

    #[test]
    fn panes_sit_end_to_end_until_they_stick() {
        let g = geo(4, 1200.);
        assert_eq!(g.left(0, 0.), 0.);
        assert_eq!(g.left(1, 0.), 560.);
        assert_eq!(g.left(2, 0.), 1120.);
        // Scrolled 600: pane 1 would be at -40, so it sticks at 40.
        assert_eq!(g.left(1, 600.), 40.);
        assert_eq!(g.left(2, 600.), 520.);
        assert_eq!(g.max_scroll(), 4. * 560. - 1200.);
    }

    #[test]
    fn a_pane_becomes_a_spine_below_the_threshold() {
        let g = geo(3, 1200.);
        // Pane 0 shows 560 − scroll until pane 1 reaches 40.
        assert!(!g.is_spine(0, 470.)); // 90 shows
        assert!(g.is_spine(0, 490.)); // 70 shows
        assert!(!g.is_spine(2, 10_000.), "the last pane is never a spine");
    }

    #[test]
    fn the_spine_click_scrolls_to_index_times_pane_less_spine() {
        let g = geo(5, 1200.);
        assert_eq!(g.spine_target(1), 520.);
        assert_eq!(g.spine_target(2), 1040.);
        // At that scroll the pane stands whole at its sticky place.
        assert_eq!(g.left(2, 1040.), 80.);
        assert_eq!(g.visible(2, 1040.), 560.);
    }

    #[test]
    fn reveal_brings_a_pane_fully_into_view_and_no_further() {
        let g = geo(6, 1200.);
        // Already whole: no movement.
        let s = 600.;
        assert_eq!(g.reveal(1, s), s.min(520.));
        // The last pane: scroll to the end.
        assert_eq!(g.reveal(5, 0.), g.max_scroll());
        let at = g.reveal(5, 0.);
        assert!(g.left(5, at) + 560. <= 1200.);
        // An old pane from the end: its spine opens.
        let back = g.reveal(1, g.max_scroll());
        assert_eq!(g.visible(1, back), 560.);
    }

    #[test]
    fn a_deep_stack_in_a_narrow_window_still_shows_the_newest_pane() {
        // 960 window − 264 sidebar ≈ 696 for the scroller; nine panes need 880 for spines.
        let g = geo(9, 696.);
        let end = g.max_scroll();
        assert!(g.left(8, end) + 560. <= 696. + 0.01, "{}", g.left(8, end));
        assert_eq!(g.visible(8, end), 560.);
        // The oldest spines fold under one another at the left edge, never off it.
        assert_eq!(g.stuck(0), 0.);
        assert!(g.stuck(1) >= 0.);
        // A spine click still stands its pane whole inside the scroller.
        let at = g.spine_target(7);
        assert_eq!(g.visible(7, at), 560.);
        assert!(g.left(7, at) + 560. <= 696. + 0.01);
    }

    #[test]
    fn panes_past_the_right_edge_are_off_screen() {
        let g = geo(4, 1000.);
        assert!(!g.off_screen(1, 0.));
        assert!(g.off_screen(2, 0.));
        assert!(!g.off_screen(2, 200.));
    }

    #[test]
    fn a_scroller_narrower_than_a_pane_shrinks_the_pane_to_fit() {
        // 960 window − 264 left sidebar − 380 right sidebar = 316 for the scroller.
        let g = Geometry::fit(560., 40., 80., 316., 4);
        assert_eq!(g.pane, 276., "the pane leaves room for one spine");
        let end = g.max_scroll();
        assert_eq!(g.left(3, end) + g.pane, 316.);
        assert_eq!(g.visible(3, end), 276.);
        assert!(g.is_spine(2, end));
        assert_eq!(g.visible(2, end), 40., "the previous pane shows as a spine");
        // One entry has no spine to show: the pane takes the whole scroller.
        assert_eq!(Geometry::fit(560., 40., 80., 316., 1).pane, 316.);
        for i in 0..4 {
            let at = g.spine_target(i);
            assert!(g.left(i, at) >= 0.);
            assert!(g.left(i, at) + g.pane <= 316. + 0.01, "pane {i}");
        }
        // A wide scroller keeps the 560 px pane (§4.3).
        assert_eq!(Geometry::fit(560., 40., 80., 1200., 4).pane, 560.);
    }

    #[test]
    fn a_merge_onto_an_open_page_leaves_one_copy() {
        // Panes: [Daily, a, b, a] after pane 3 merged into a: the older a goes, pane 3
        // (the one in use) stays and keeps focus at its new index.
        let s = vec![Entry::Daily, p("a"), p("b"), p("a")];
        let plan = dedupe(&s, ViewMode::Panes, 3).unwrap();
        assert_eq!(
            plan.apply(&s, Entry::clone),
            vec![Entry::Daily, p("b"), p("a")]
        );
        assert_eq!(plan.focus, 2);
        // The twin is newer: it goes, and pane 1 keeps its place and focus.
        let plan = dedupe(&s, ViewMode::Panes, 1).unwrap();
        assert_eq!(
            plan.apply(&s, Entry::clone),
            vec![Entry::Daily, p("a"), p("b")]
        );
        assert_eq!(plan.focus, 1);
        // Single: the shown entry stays the last one.
        let plan = dedupe(&s, ViewMode::Single, 3).unwrap();
        assert_eq!(
            plan.apply(&s, Entry::clone),
            vec![Entry::Daily, p("b"), p("a")]
        );
        // No twin, or Daily Notes: nothing to do.
        assert!(dedupe(&abc(), ViewMode::Panes, 2).is_none());
        assert!(dedupe(&abc(), ViewMode::Panes, 0).is_none());
        // A zoom is part of the entry: the same page zoomed elsewhere is not a twin.
        let z = Entry::Page {
            page: PageId("a".into()),
            zoom: Some(BlockId("b1".into())),
        };
        assert!(dedupe(&[Entry::Daily, p("a"), z], ViewMode::Panes, 2).is_none());
    }
}
