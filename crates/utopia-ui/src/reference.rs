//! One reference row (FR-3.1): the path above it, a bullet, the referencing block rendered
//! the way an outline row is — marks, page chips and `((block))` chips inside one shaped
//! text (§13.4) — and its first children inline (LR-4). Linked References and the right
//! sidebar's References tab share it (RS-1), so a reference reads the same wherever it is
//! shown and the two cannot drift.

use std::ops::Range;

use gpui::{div, prelude::*, px, AnyElement, Context, Entity, MouseButton, StyledText, Window};
use utopia_core::{BlockId, PageId};
use utopia_store::{BlockRefStatus, RefHit, Store};

use crate::rich::{self, ChipState, LinkTarget};
use crate::theme::{Theme, CAPTION_SIZE, REFERENCE};

/// How a `((uid))` chip should look in a row of `store` (FR-4.3, FR-4.7, AC-4.5).
pub fn chip_state(store: &Store, uid: &str, row_block: &BlockId) -> ChipState {
    if uid == row_block.as_str() {
        return ChipState::Cyclic;
    }
    match store.block_ref_status(uid) {
        Ok(BlockRefStatus::Live { text, .. }) => ChipState::Live(text),
        Ok(BlockRefStatus::Deleted { .. }) => ChipState::Deleted,
        _ => ChipState::Missing,
    }
}

/// What a row shows besides the referencing block: the page it sits on, for the first part of
/// its path (LR-3), and the children drawn under it (LR-4).
pub struct RefRow<'a> {
    pub hit: &'a RefHit,
    /// The referencing page's title (the group header's, LR-2).
    pub page_title: &'a str,
    /// The first children of the referencing block and how many more there are (LR-4).
    pub children: &'a [(BlockId, String)],
    pub hidden: usize,
}

impl<'a> RefRow<'a> {
    /// A row on its own, with no inline children: what the right sidebar's References tab
    /// shows (RS-1). The group above it already names the page.
    pub fn alone(hit: &'a RefHit, page_title: &'a str) -> Self {
        Self {
            hit,
            page_title,
            children: &[],
            hidden: 0,
        }
    }
}

/// What a row's clicks do. A page view and the workspace each have their own idea of
/// "open this page here" (a `PageView` emits an event, the workspace switches panes), so the
/// row calls back into the view that hosts it instead of hard-coding one.
pub trait RefRowHost: Sized {
    /// The row itself was clicked: show `page` with `block` focused.
    fn open_reference(
        &mut self,
        page: PageId,
        block: BlockId,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// A path part above the row was clicked (LR-3): show `page`, focused on `block` when
    /// the part names an ancestor.
    fn open_reference_page(
        &mut self,
        page: PageId,
        block: Option<BlockId>,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// A chip inside the row was clicked: follow the page/block it names.
    fn follow_reference(
        &mut self,
        target: LinkTarget,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// The "+N more" chip was clicked (LR-4): show the rest of the block's children. A host
    /// that never draws the chip (the sidebar's References tab) does nothing.
    fn expand_reference_children(
        &mut self,
        _block: &BlockId,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
}

/// One block body, shaped and bound to its layout so a click can tell a chip from prose.
struct Body {
    element: AnyElement,
    layout: gpui::TextLayout,
    rendered: rich::Rendered,
}

fn body(text: &str, block: &BlockId, theme: &Theme, store: &Store) -> Body {
    let rendered = rich::render(text, theme, &mut |uid| chip_state(store, uid, block));
    let styled =
        StyledText::new(rendered.text.clone()).with_highlights(rendered.highlights.clone());
    let layout = styled.layout().clone();
    let links: Vec<Range<usize>> = rendered.links.iter().map(|(r, _)| r.clone()).collect();
    let chips = rendered.chips.clone();
    Body {
        element: div()
            .flex_1()
            .min_w_0()
            .child(rich::LinkText::new(styled, links).with_chips(chips, theme.tag_bg))
            .into_any_element(),
        layout,
        rendered,
    }
}

/// A reference row. `prefix` + `ix` name the block's body for `debug_selector` (tests read
/// its bounds) and the row itself as `{prefix}-row-{ix}`; `pad_x` is the row's inset from
/// the panel edge, so the two call sites can keep their own measure without a second
/// renderer.
pub fn reference_row<H: RefRowHost + 'static>(
    prefix: &'static str,
    ix: usize,
    row: RefRow<'_>,
    theme: &Theme,
    store: &Store,
    pad_x: f32,
    host: Entity<H>,
) -> AnyElement {
    let th = theme.clone();
    let hit = row.hit;
    let (page, block) = (hit.page.clone(), hit.block.clone());
    let bullet = || div().w(px(14.)).text_color(th.bullet).child("•");

    // LR-3: the full path above the block, page first, each part clickable. A block that
    // sits directly on its page has no ancestors, and the group header right above already
    // names that page, so it gets no path line.
    let mut path = div()
        .debug_selector(|| format!("hit-path-{ix}"))
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .text_size(px(CAPTION_SIZE))
        .line_height(px(crate::theme::SMALL_TIGHT.line))
        .text_color(th.muted);
    if !hit.breadcrumb.is_empty() {
        let target = page.clone();
        let part_host = host.clone();
        let page_title = row.page_title.to_string();
        path = path.child(
            div()
                .id(("path-page", ix))
                .debug_selector(|| format!("hit-path-part-{ix}-0"))
                .cursor_pointer()
                .hover(|s| s.text_color(th.link))
                .child(page_title)
                .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                    // The row under the path opens the block; the page part opens the page.
                    cx.stop_propagation();
                    part_host.update(cx, |this, cx| {
                        this.open_reference_page(
                            target.clone(),
                            None,
                            e.modifiers.shift,
                            window,
                            cx,
                        )
                    });
                }),
        );
        for (i, (ancestor, caption)) in hit.breadcrumb.iter().enumerate() {
            let label: String = caption.chars().take(40).collect();
            let label = if label.is_empty() {
                "(empty)".to_string()
            } else {
                label
            };
            let target = page.clone();
            let ancestor = ancestor.clone();
            let part_host = host.clone();
            path = path.child(div().child("›")).child(
                div()
                    .id(("path-part", ix * 100 + i))
                    .debug_selector(|| format!("hit-path-part-{ix}-{}", i + 1))
                    .cursor_pointer()
                    .hover(|s| s.text_color(th.link))
                    .child(label)
                    .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                        cx.stop_propagation();
                        part_host.update(cx, |this, cx| {
                            this.open_reference_page(
                                target.clone(),
                                Some(ancestor.clone()),
                                e.modifiers.shift,
                                window,
                                cx,
                            )
                        });
                    }),
            );
        }
    }

    let main_body = body(&hit.text, &hit.block, &th, store);
    let (row_host, body_host) = (host.clone(), host.clone());

    // LR-4: the first children inline, then a chip that opens the rest. Each child is its
    // own shaped text, so it gets its own hit-testing: a link inside it is followed, and a
    // click on anything else falls through to the row, which opens the referencing block.
    let mut kids = div().flex().flex_col();
    for (i, (child, child_text)) in row.children.iter().enumerate() {
        let child_body = body(child_text, child, &th, store);
        let child_host = host.clone();
        kids = kids.child(
            div()
                .debug_selector(|| format!("hit-child-{ix}-{i}"))
                .flex()
                .ml(px(14.))
                .child(bullet())
                .child(child_body.element)
                .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                    let Ok(i) = child_body.layout.index_for_position(e.position) else {
                        return;
                    };
                    let Some(target) = child_body.rendered.link_at(i).cloned() else {
                        return;
                    };
                    cx.stop_propagation();
                    child_host.update(cx, |this, cx| {
                        this.follow_reference(target, e.modifiers.shift, window, cx)
                    });
                }),
        );
    }
    let more = (row.hidden > 0).then(|| {
        let block = block.clone();
        div()
            .id(("hit-more", ix))
            .debug_selector(|| format!("hit-more-{ix}-{}", row.hidden))
            .ml(px(28.))
            .px_2()
            .rounded(px(crate::theme::RADIUS_CONTROL))
            .text_size(px(CAPTION_SIZE))
            .text_color(th.link)
            .cursor_pointer()
            .hover(|s| s.bg(th.hover))
            .child(format!("+{} more", row.hidden))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                // The chip is inside the hit row, whose click opens the block.
                cx.stop_propagation();
                body_host.update(cx, |this, cx| {
                    this.expand_reference_children(&block, window, cx)
                });
            })
    });

    div()
        .id(("reference", ix))
        .debug_selector(move || format!("{prefix}-row-{ix}"))
        // DESIGN.md §2.2/§3.7: a referenced block is one step below body text.
        .text_size(px(REFERENCE.size))
        .line_height(px(REFERENCE.line))
        .px(px(pad_x))
        .py_0p5()
        .rounded(px(crate::theme::RADIUS_CHIP))
        .flex()
        .flex_col()
        .cursor_pointer()
        .hover(|d| d.bg(th.hover))
        .when(!hit.breadcrumb.is_empty(), |d| d.child(path))
        .child(
            div()
                .flex()
                .child(bullet())
                // A flex item's minimum width is its unwrapped text unless `min_w_0`, so
                // the body would run wide instead of wrapping at the panel's edge.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .debug_selector(|| format!("{prefix}-{ix}"))
                        .child(main_body.element),
                ),
        )
        .when(!row.children.is_empty(), |d| d.child(kids))
        .children(more)
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            if let Ok(i) = main_body.layout.index_for_position(e.position) {
                if let Some(target) = main_body.rendered.link_at(i).cloned() {
                    cx.stop_propagation();
                    row_host.update(cx, |this, cx| {
                        this.follow_reference(target, e.modifiers.shift, window, cx)
                    });
                    return;
                }
            }
            row_host.update(cx, |this, cx| {
                this.open_reference(page.clone(), block.clone(), e.modifiers.shift, window, cx)
            });
        })
        .into_any_element()
}
