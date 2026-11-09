//! Month calendar used as the journal date picker (FR-1.3, FR-1.4): the page title's
//! popover grid ([`render`]) and the left sidebar's quiet month strip ([`render_month_strip`]).

use std::collections::HashSet;

use chrono::{Datelike, Months, NaiveDate};
use gpui::{div, prelude::*, px, Context, Div, MouseButton, MouseDownEvent, Window};

use crate::theme::theme;

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

/// The 6×7 grid of dates shown for `month` (Monday first).
pub fn grid(month: NaiveDate) -> Vec<NaiveDate> {
    let first = first_of_month(month);
    let lead = first.weekday().num_days_from_monday() as u64;
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

/// The callbacks the sidebar's month strip needs, bundled so the render signature stays
/// readable (pick a day, move a month, fold the strip).
pub struct StripActions<V> {
    pub pick: fn(&mut V, NaiveDate, bool, &mut Window, &mut Context<V>),
    pub shift: fn(&mut V, i32, &mut Context<V>),
    pub toggle: fn(&mut V, &mut Context<V>),
}

// Function pointers are `Copy` whatever `V` is, so each listener can take its own copy.
impl<V> Clone for StripActions<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for StripActions<V> {}

/// The left sidebar's month strip (SB-5): a quiet 7-column grid under Journal. The month
/// name is the collapse control — one click folds the grid away; the arrows still move
/// between months either way. Days with an entry get a small dot under the number, and
/// today is drawn in the link colour. Display only: the days are existing daily pages
/// (§5.5, rule 6), nothing is created by looking at the month.
pub fn render_month_strip<V: 'static>(
    cx: &mut Context<V>,
    month: NaiveDate,
    today: NaiveDate,
    existing: &HashSet<NaiveDate>,
    open: bool,
    actions: StripActions<V>,
) -> Div {
    let th = theme(cx).clone();
    let arrow = |id: &'static str, glyph: &'static str, delta: i32| {
        div()
            .id(id)
            .px_1()
            .rounded(px(crate::theme::RADIUS_CHIP))
            .cursor_pointer()
            .text_size(px(crate::theme::CAPTION_SIZE))
            .text_color(th.muted)
            .hover(|d| d.bg(th.hover).text_color(th.ink))
            .child(glyph)
            .on_click(cx.listener(move |v, _, _, cx| (actions.shift)(v, delta, cx)))
    };
    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(arrow("cal-prev", "‹", -1))
        .child(
            div()
                .id("toggle-calendar")
                .debug_selector(|| "toggle-calendar".into())
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded(px(crate::theme::RADIUS_CHIP))
                .cursor_pointer()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .hover(|d| d.bg(th.hover).text_color(th.ink))
                .child(if open { "▾" } else { "▸" })
                .child(month.format("%B %Y").to_string())
                .on_click(cx.listener(move |v, _, _, cx| (actions.toggle)(v, cx))),
        )
        .child(arrow("cal-next", "›", 1));
    let mut strip = div().flex().flex_col().gap_0p5().child(header);
    if !open {
        return strip;
    }
    let names = div()
        .flex()
        .pt_1()
        .children(["M", "T", "W", "T", "F", "S", "S"].iter().map(|n| {
            div()
                .flex_1()
                .flex()
                .justify_center()
                .text_size(px(crate::theme::CAPTION_SIZE))
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
                .flex()
                .flex_col()
                .items_center()
                .rounded(px(crate::theme::RADIUS_CHIP))
                .cursor_pointer()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .line_height(px(15.))
                .text_color(if is_today {
                    th.link
                } else if in_month {
                    th.ink_2
                } else {
                    th.muted
                })
                .when(is_today, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                .hover(|d| d.bg(th.hover))
                .child(d.day().to_string())
                .child(div().h(px(5.)).flex().items_center().when(has, |d| {
                    d.child(
                        div()
                            .debug_selector(move || format!("cal-dot-{dot_iso}"))
                            .size(px(3.))
                            .rounded_full()
                            .bg(if is_today { th.link } else { th.muted }),
                    )
                }))
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
    strip = strip.child(
        div()
            .debug_selector(|| "left-calendar".into())
            .px_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(names)
            .children(rows),
    );
    strip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_starts_on_monday_and_covers_month() {
        let nov = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let g = grid(nov);
        assert_eq!(g.len(), 42);
        assert_eq!(g[0].weekday(), chrono::Weekday::Mon);
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
