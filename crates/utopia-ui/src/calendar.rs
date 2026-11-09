//! Month calendar used as the journal date picker (FR-1.3, FR-1.4): the page title's
//! popover grid ([`render`]) and the left sidebar's quiet month strip ([`render_month_strip`]).

use std::collections::HashSet;

use chrono::{Datelike, Months, NaiveDate};
use gpui::{div, prelude::*, px, Context, Div, MouseButton, MouseDownEvent, Window};

use crate::icons::Icon;
use crate::theme::{
    ghost_icon, theme, CALENDAR_CELL_HEIGHT, CALENDAR_DOT, LABEL_MONO, SIDEBAR_LABEL,
};

/// The nearest thing to an accessible label for an icon-only button: gpui 0.2 has no
/// accessibility tree, so the calendar's arrows label themselves with a tooltip.
struct CalTip(&'static str);

impl Render for CalTip {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        div()
            .debug_selector(|| "tip".into())
            .px_2()
            .py_1()
            .rounded(px(crate::theme::RADIUS_CONTROL))
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .text_size(px(crate::theme::CAPTION_SIZE))
            .text_color(th.ink)
            .child(self.0)
    }
}

fn cal_tip(text: &'static str) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView {
    move |_, cx| cx.new(|_| CalTip(text)).into()
}

pub fn first_of_month(d: NaiveDate) -> NaiveDate {
    d.with_day(1).expect("day 1 exists")
}

pub fn shift_month(d: NaiveDate, delta: i32) -> NaiveDate {
    let m = first_of_month(d);
    if delta >= 0 {
        m + Months::new(delta as u32)
    } else {
        m - Months::new((-delta) as u32)
    }
}

/// The 6×7 grid of dates shown for `month`. Weeks start on Sunday (DESIGN.md §3.2).
pub fn grid(month: NaiveDate) -> Vec<NaiveDate> {
    let first = first_of_month(month);
    let lead = first.weekday().num_days_from_sunday() as u64;
    let start = first - chrono::Days::new(lead);
    (0..42).map(|i| start + chrono::Days::new(i)).collect()
}

pub fn render<V: 'static>(
    cx: &mut Context<V>,
    month: NaiveDate,
    today: NaiveDate,
    existing: &HashSet<NaiveDate>,
    pick: fn(&mut V, NaiveDate, bool, &mut Window, &mut Context<V>),
    shift: fn(&mut V, i32, &mut Context<V>),
) -> Div {
    let th = theme(cx).clone();
    let header = div()
        .flex()
        .justify_between()
        .items_center()
        .text_size(px(crate::theme::BODY_SIZE))
        .child(
            div()
                .id("cal-prev")
                .px_1()
                .cursor_pointer()
                .child("‹")
                .on_click(cx.listener(move |v, _, _, cx| shift(v, -1, cx))),
        )
        .child(
            div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(month.format("%B %Y").to_string()),
        )
        .child(
            div()
                .id("cal-next")
                .px_1()
                .cursor_pointer()
                .child("›")
                .on_click(cx.listener(move |v, _, _, cx| shift(v, 1, cx))),
        );
    let names = div()
        .flex()
        .children(["M", "T", "W", "T", "F", "S", "S"].iter().map(|n| {
            div()
                .w(px(26.))
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .flex()
                .justify_center()
                .child(*n)
        }));
    let cells = grid(month).into_iter().map(|d| {
        let in_month = d.month() == month.month();
        let is_today = d == today;
        let has = existing.contains(&d);
        let iso = d.format("%Y-%m-%d").to_string();
        div()
            // The strip's cells are `cal-<iso>` (a click target for tests); the popover's
            // share the grid but get their own name so both can be open at once.
            .debug_selector(move || format!("cal-pop-{iso}"))
            .w(px(26.))
            .h(px(22.))
            .flex()
            .justify_center()
            .items_center()
            .text_size(px(crate::theme::CAPTION_SIZE))
            .rounded(px(crate::theme::RADIUS_CHIP))
            .cursor_pointer()
            .text_color(if in_month { th.ink } else { th.muted })
            .when(has, |c| c.font_weight(gpui::FontWeight::BOLD))
            .when(is_today, |c| c.bg(th.line_soft).text_color(th.link))
            .hover(|s| s.bg(th.hover))
            .child(d.day().to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                    pick(v, d, e.modifiers.shift, w, cx)
                }),
            )
    });
    let mut rows = Vec::new();
    let cells: Vec<_> = cells.collect();
    let mut it = cells.into_iter();
    for _ in 0..6 {
        rows.push(div().flex().children(it.by_ref().take(7)));
    }
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(header)
        .child(names)
        .children(rows)
}

/// The callbacks the sidebar's mini calendar needs, bundled so the render signature stays
/// readable: open a day, move a month.
pub struct StripActions<V> {
    pub pick: fn(&mut V, NaiveDate, bool, &mut Window, &mut Context<V>),
    pub shift: fn(&mut V, i32, &mut Context<V>),
}

// Function pointers are `Copy` whatever `V` is, so each listener can take its own copy.
impl<V> Clone for StripActions<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for StripActions<V> {}

/// The sidebar's mini calendar (DESIGN.md §3.2): month label with prev/next buttons, a
/// Sunday-first 7-column grid, mono weekday labels, 30 px day cells. Today is a solid
/// `link` cell with white text; a day with an entry carries a 4 px `link` dot under its
/// number. Clicking a day opens that daily note. Display only: the days are existing daily
/// pages (§5.5, rule 6), nothing is created by looking at the month.
pub fn render_month_strip<V: 'static>(
    cx: &mut Context<V>,
    month: NaiveDate,
    today: NaiveDate,
    existing: &HashSet<NaiveDate>,
    actions: StripActions<V>,
) -> Div {
    let th = theme(cx).clone();
    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            ghost_icon("cal-prev", Icon::ChevronLeft, &th)
                .tooltip(cal_tip("Previous month"))
                .on_click(cx.listener(move |v, _, _, cx| (actions.shift)(v, -1, cx))),
        )
        .child(
            div()
                .debug_selector(|| "cal-month".into())
                .text_size(px(crate::theme::SMALL.size))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(th.ink)
                .child(month.format("%B %Y").to_string()),
        )
        .child(
            ghost_icon("cal-next", Icon::ChevronRight, &th)
                .tooltip(cal_tip("Next month"))
                .on_click(cx.listener(move |v, _, _, cx| (actions.shift)(v, 1, cx))),
        );
    let names = div()
        .flex()
        .children(["S", "M", "T", "W", "T", "F", "S"].iter().map(|n| {
            div()
                .flex_1()
                .flex()
                .justify_center()
                .font_family(LABEL_MONO)
                .text_size(px(SIDEBAR_LABEL.size))
                .text_color(th.muted)
                .child(*n)
        }));
    let cells: Vec<_> = grid(month)
        .into_iter()
        .map(|d| {
            let in_month = d.month() == month.month();
            let is_today = d == today;
            let has = existing.contains(&d);
            let iso = d.format("%Y-%m-%d").to_string();
            let dot_iso = iso.clone();
            div()
                .id(gpui::ElementId::Name(iso.clone().into()))
                .debug_selector(move || format!("cal-{iso}"))
                .flex_1()
                .h(px(CALENDAR_CELL_HEIGHT))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .rounded(px(crate::theme::RADIUS_CONTROL))
                .cursor_pointer()
                .text_size(px(crate::theme::SMALL.size))
                .line_height(px(crate::theme::SMALL_TIGHT.line))
                .text_color(if is_today {
                    th.paper
                } else if in_month {
                    th.ink
                } else {
                    th.muted
                })
                .when(is_today, |d| {
                    d.bg(th.link).font_weight(gpui::FontWeight::MEDIUM)
                })
                .when(!is_today, |d| d.hover(|s| s.bg(th.hover)))
                .child(d.day().to_string())
                .child(
                    div()
                        .h(px(CALENDAR_DOT + 2.))
                        .flex()
                        .items_center()
                        .when(has, |d| {
                            d.child(
                                div()
                                    .debug_selector(move || format!("cal-dot-{dot_iso}"))
                                    .size(px(CALENDAR_DOT))
                                    .rounded_full()
                                    .bg(if is_today { th.paper } else { th.link }),
                            )
                        }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                        (actions.pick)(v, d, e.modifiers.shift, w, cx)
                    }),
                )
        })
        .collect();
    let mut rows = Vec::new();
    let mut it = cells.into_iter();
    for _ in 0..6 {
        rows.push(div().flex().children(it.by_ref().take(7)));
    }
    div().flex().flex_col().gap_0p5().child(header).child(
        div()
            .debug_selector(|| "left-calendar".into())
            .flex()
            .flex_col()
            .gap_0p5()
            .child(names)
            .children(rows),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_starts_on_sunday_and_covers_month() {
        let nov = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let g = grid(nov);
        assert_eq!(g.len(), 42);
        assert_eq!(g[0].weekday(), chrono::Weekday::Sun);
        assert!(g.contains(&NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()));
        assert!(g.contains(&NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()));
        assert_eq!(
            shift_month(nov, -11),
            NaiveDate::from_ymd_opt(2025, 12, 1).unwrap()
        );
        assert_eq!(
            shift_month(nov, 2),
            NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()
        );
    }
}
