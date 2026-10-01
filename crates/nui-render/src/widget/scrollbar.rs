//! Where a scrollbar's thumb sits, and how tall it is.
//!
//! A scrollbar is not a control and not an element: it is the *picture* of
//! a decision the runtime already made. `nui_runtime::widget::max_scroll_y`
//! answers "how far can this travel?", the wheel handler clamps to it, and
//! this module turns that number plus the current offset into a rectangle.
//!
//! # Why it lives here and not in the tree
//!
//! The alternative is an engine-generated `Scrollbar` child element, which
//! buys styling and an `id` at the cost of touching four subsystems
//! (instantiation, layout, hit testing, input) — and of competing with the
//! `ListView` virtualizer's leading `Spacer` for the container's child
//! list. Nothing in the current requirement needs either style or `id`, so
//! the thumb is appended to the draw list during the scene walk, at the
//! one place that already knows the container's box, its scroll offset and
//! its clip. The decision is recorded as **D36** in `plan.md`.
//!
//! # Why a pure function
//!
//! The geometry has real edge cases (a list whose thumb would be
//! sub-pixel, a container shorter than the minimum thumb, an offset past
//! the limit) and the scene builder is a poor place to test them — it
//! needs the whole tree plus a text system. Keeping the arithmetic here
//! means every one of those cases is a two-line test.

use nui_core::{Color, Point, Rect, Size};

use nui_runtime::Element;

use crate::props::color_property;

/// The shortest a thumb is allowed to be, in dp.
///
/// A hundred-thousand-row list gives a thumb height of `viewport² /
/// content`, which for a 400dp viewport is under a hundredth of a dp —
/// invisible, and worse than useless because the user reads its absence as
/// "there is nowhere to scroll". Clamping to a visible minimum is the same
/// thing every native toolkit does.
pub const MIN_THUMB: f32 = 24.0;

/// The thumb is inset from the container's right edge by this much, so it
/// reads as floating over the content rather than as part of its border.
pub const THUMB_INSET: f32 = 2.0;

/// Width of the thumb, in dp.
pub const THUMB_WIDTH: f32 = 4.0;

/// Corner radius of the thumb: a half-round capsule.
pub const THUMB_RADIUS: f32 = THUMB_WIDTH / 2.0;

/// How far to the *left* of the thumb a pointer still counts as "on the
/// bar", in dp.
///
/// The thumb is 4dp wide — far below the ~44dp a comfortable pointer target
/// wants — so the hit strip is widened on the content side only. The right
/// side needs no slop: it is already at the container's edge, and anything
/// past it belongs to whatever is beside the container.
pub const HIT_SLOP: f32 = 6.0;

/// The default thumb color, for a container that declares no `fill`.
///
/// Mid grey at half alpha: it reads as a scrollbar over both a light and a
/// dark container, which a black or white thumb does not.
const DEFAULT_THUMB: Color = Color::from_rgba(0.53, 0.53, 0.53, 0.5);

/// Alpha of a thumb derived from the container's own `fill`.
const DERIVED_ALPHA: f32 = 0.35;

/// How far a derived thumb is pushed from the container's `fill` before it
/// is used, as a fraction of the way to black or white.
///
/// A pure inversion looks wrong on a mid-grey container (the thumb lands on
/// a nearly identical grey), so the distance is what matters rather than
/// the direction. `lighten`/`darken` by a fixed step keeps the thumb
/// recognisably the container's colour while guaranteeing separation —
/// the same trick `Palette::of` uses for a custom `fill`'s hover state.
const DERIVED_STEP: f32 = 0.45;

/// The thumb color for a scroll container.
///
/// A container that declares a `fill` gets a thumb derived from it; one
/// that declares none gets [`DEFAULT_THUMB`]. Deriving rather than
/// hard-coding is what keeps a themed list (the gallery's dark pages, a
/// light card) from having a scrollbar that belongs to some other palette.
pub fn thumb_color(element: &Element) -> Color {
    let Some(fill) = color_property(element, "fill") else {
        return DEFAULT_THUMB;
    };
    let (r, g, b) = (fill.red(), fill.green(), fill.blue());
    // Perceived luminance (Rec. 601) decides which way to push.
    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
    let base = if luminance > 0.5 {
        crate::widget::darken(fill, DERIVED_STEP)
    } else {
        crate::widget::lighten(fill, DERIVED_STEP)
    };
    return base.with_alpha(DERIVED_ALPHA);
}

/// A scrollbar resolved into the two rectangles that paint it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarMetrics {
    /// The full-height lane the thumb slides in, in window space. Currently
    /// the container's own box — a track distinct from the container would
    /// need a `scrollbar_track` property, which nothing asks for.
    pub track: Rect,
    /// The thumb, in window space.
    pub thumb: Rect,
}

/// The two numbers a scrollbar's placement needs, derived from the lane and
/// the limit.
///
/// Both directions of the mapping — drawing the thumb from an offset, and
/// turning a dropped thumb back into an offset — go through this, which is
/// the only way to guarantee the grab area and the drawn thumb cannot
/// drift apart. A second copy of the arithmetic in the drag handler is
/// exactly the kind of divergence that reads as "the bar fights the mouse".
struct ThumbTravel {
    /// Height of the thumb, after the `MIN_THUMB` clamp.
    thumb_height: f32,
    /// How far the thumb's top edge may move: `viewport - thumb_height`.
    ///
    /// **Zero is meaningful**: it is a lane no taller than the minimum
    /// thumb (a 10dp-tall container), where the thumb *is* the lane. Such a
    /// container still has a scrollbar — it has content to scroll, after
    /// all — but its thumb has nowhere to travel, so its progress is pinned
    /// to zero and it cannot be dragged. The arithmetic below divides by
    /// this, so every use is guarded.
    travel: f32,
}

/// Resolves the thumb geometry both directions need, or `None` when the
/// lane cannot carry a scrollbar.
///
/// The guards live here rather than in the callers so both the painter and
/// the drag handler agree on every degenerate case:
///
/// - no overflow (`max_scroll_y <= 0`), which is also what an empty list,
///   a missing model and content shorter than the viewport all report;
/// - a lane with no height or no width, which has nowhere to put a thumb.
fn thumb_travel(viewport: Size, max_scroll_y: f32) -> Option<ThumbTravel> {
    // Nothing to indicate. This is also the guard that keeps the division
    // below off a zero divisor, and it is deliberately the *limit* (not
    // `content - viewport`) so it matches the wheel's clamp exactly.
    if !max_scroll_y.is_finite() || max_scroll_y <= 0.0 {
        return None;
    }
    // A lane with no extent cannot hold a thumb. `is_finite` also rejects
    // NaN, which would otherwise fail every comparison silently and let a
    // NaN lane through into the arithmetic below.
    if !viewport.width.is_finite() || !viewport.height.is_finite() {
        return None;
    }
    if viewport.width <= 0.0 || viewport.height <= 0.0 {
        return None;
    }
    // How much of the content is on screen. `max_scroll_y` is
    // `content - viewport`, so this is exact rather than an estimate: for a
    // ListView it is `viewport / (rows * row_height)`, for a Scroll the
    // measured child extent. Both come from the same `content_height` the
    // limit does.
    let content_height = viewport.height + max_scroll_y;
    let visible_ratio = (viewport.height / content_height).clamp(0.0, 1.0);
    // The thumb never exceeds the lane: for a container shorter than
    // `MIN_THUMB` the minimum *is* the lane, or the thumb would overflow
    // the element it belongs to.
    let thumb_height =
        (viewport.height * visible_ratio).clamp(MIN_THUMB.min(viewport.height), viewport.height);
    return Some(ThumbTravel {
        thumb_height,
        travel: viewport.height - thumb_height,
    });
}

/// Resolves a scrollbar's geometry, or `None` when there is nothing to
/// show.
///
/// `origin` and `viewport` are the container's top-left and size in window
/// space (the caller has already applied ancestor scroll and layout).
/// `scroll_y` is the current offset and `max_scroll_y` the travel limit —
/// **the same value the wheel handler clamps against**, so the thumb and
/// the scroll position can never disagree about where "the bottom" is.
pub fn scrollbar_metrics(
    origin: Point,
    viewport: Size,
    scroll_y: f32,
    max_scroll_y: f32,
) -> Option<ScrollbarMetrics> {
    let geometry = thumb_travel(viewport, max_scroll_y)?;
    // An offset past the limit is clamped rather than trusted: the limit can
    // shrink under the offset (a row deleted from the model while scrolled
    // to the bottom) and the thumb must not slide out of its lane when it
    // does.
    let progress = (scroll_y / max_scroll_y).clamp(0.0, 1.0);
    let thumb_top = origin.y + progress * geometry.travel;
    let thumb_left = origin.x + viewport.width - THUMB_INSET - THUMB_WIDTH;
    return Some(ScrollbarMetrics {
        track: Rect::new(origin, viewport),
        thumb: Rect::new(
            Point::new(thumb_left, thumb_top),
            Size::new(THUMB_WIDTH, geometry.thumb_height),
        ),
    });
}

/// The inverse of [`scrollbar_metrics`]: the `scroll_y` a given thumb top
/// edge stands for.
///
/// The drag handler needs this. `thumb_top` is the thumb's **top edge in
/// window space** (not the pointer position — the caller subtracts the
/// grab offset it recorded at press time, so the thumb does not jump to
/// centre itself under the cursor).
///
/// Rounds through the same [`ThumbTravel`] the painter uses, so a drag to
/// the very bottom of the lane lands exactly on `max_scroll_y` rather than
/// a hair short — which, over a long list, is the difference between
/// showing the last row and showing the last row minus a pixel.
///
/// `None` mirrors `scrollbar_metrics`: a container that draws no thumb
/// cannot be dragged by one.
pub fn scroll_y_from_thumb_top(viewport: Size, thumb_top: f32, max_scroll_y: f32) -> Option<f32> {
    let geometry = thumb_travel(viewport, max_scroll_y)?;
    let progress = nui_tools::progress(thumb_top, geometry.travel);
    return Some(progress * max_scroll_y);
}

/// How much of a scroll container's lane a pointer at `y` is over.
///
/// Split from the position mapping because the two answer different
/// questions and only one of them needs the geometry settled: a click on
/// the *track* but outside the thumb should page toward the pointer, and
/// that decision does not depend on how tall the thumb is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarHit {
    /// The pointer is over the thumb: grab it and drag.
    Thumb,
    /// The pointer is over the lane but not the thumb: page toward it.
    Track,
}

/// Whether a pointer at `position` lands on the scrollbar of a container
/// whose box is `origin`/`viewport`, and where.
///
/// `None` — not on the bar at all — also covers every container that has no
/// scrollbar to hit, because the lane tests run after the geometry
/// resolves. A caller can therefore pass every scroll container it walks
/// past without first asking whether one is due.
///
/// The **whole lane** is grabbable, not just the thumb. A 4dp-wide thumb is
/// far below the ~44dp a comfortable pointer target wants, so demanding a
/// direct hit would make the bar feel broken; widening the hit test is the
/// standard fix and costs nothing, since the lane is otherwise inert — the
/// events over it would have gone to the content underneath.
pub fn scrollbar_hit(
    origin: Point,
    viewport: Size,
    scroll_y: f32,
    max_scroll_y: f32,
    position: Point,
) -> Option<ScrollbarHit> {
    let metrics = scrollbar_metrics(origin, viewport, scroll_y, max_scroll_y)?;
    // The grab strip: the thumb's own width, plus a margin of slop on the
    // content side so a pointer a dp or two left of the bar still catches
    // it. Never narrower than the thumb, and never so wide that it swallows
    // a generous slice of the content.
    let left = metrics.thumb.origin.x - HIT_SLOP;
    let right = metrics.thumb.origin.x + metrics.thumb.size.width;
    if position.x < left || position.x > right {
        return None;
    }
    if position.y < metrics.track.origin.y
        || position.y > metrics.track.origin.y + metrics.track.size.height
    {
        return None;
    }
    let on_thumb = position.y >= metrics.thumb.origin.y
        && position.y <= metrics.thumb.origin.y + metrics.thumb.size.height;
    return if on_thumb {
        Some(ScrollbarHit::Thumb)
    } else {
        Some(ScrollbarHit::Track)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100dp-tall container at the origin, scrolled to `scroll_y` out of
    /// `max_scroll_y`.
    fn metrics(scroll_y: f32, max_scroll_y: f32) -> Option<ScrollbarMetrics> {
        return scrollbar_metrics(Point::ZERO, Size::new(200.0, 100.0), scroll_y, max_scroll_y);
    }

    #[test]
    fn a_container_with_nothing_to_scroll_has_no_scrollbar() {
        assert_eq!(metrics(0.0, 0.0), None);
    }

    #[test]
    fn a_negative_or_non_finite_limit_has_no_scrollbar() {
        // Defensive: `max_scroll_y` already floors at 0, but a caller that
        // passed a raw `content - viewport` could hand one over.
        assert_eq!(metrics(0.0, -10.0), None);
        assert_eq!(metrics(0.0, f32::INFINITY), None);
        assert_eq!(metrics(0.0, f32::NAN), None);
    }

    #[test]
    fn a_container_with_no_extent_has_no_scrollbar() {
        assert_eq!(
            scrollbar_metrics(Point::ZERO, Size::new(0.0, 100.0), 0.0, 500.0),
            None
        );
        assert_eq!(
            scrollbar_metrics(Point::ZERO, Size::new(200.0, 0.0), 0.0, 500.0),
            None
        );
    }

    #[test]
    fn content_twice_the_viewport_gives_a_half_height_thumb() {
        // viewport 100, content 200 -> half the lane.
        let got = metrics(0.0, 100.0).expect("there is something to scroll");
        assert_eq!(got.thumb.size.height, 50.0);
        assert_eq!(
            got.thumb.origin.y, 0.0,
            "at the top the thumb is at the top"
        );
        assert_eq!(got.track, Rect::new(Point::ZERO, Size::new(200.0, 100.0)));
    }

    #[test]
    fn the_thumb_sits_at_the_bottom_when_scrolled_to_the_limit() {
        let got = metrics(100.0, 100.0).expect("there is something to scroll");
        // travel = 100 - 50 = 50; at progress 1 the thumb's bottom edge is
        // the lane's bottom edge.
        assert_eq!(got.thumb.origin.y, 50.0);
        assert_eq!(got.thumb.origin.y + got.thumb.size.height, 100.0);
    }

    #[test]
    fn the_thumb_is_proportional_in_the_middle() {
        let got = metrics(50.0, 100.0).expect("there is something to scroll");
        assert_eq!(got.thumb.origin.y, 25.0, "halfway along a 50dp travel");
    }

    #[test]
    fn a_very_long_list_still_has_a_visible_thumb() {
        // 100k rows of 40dp = 4e6dp of content in a 100dp viewport. The
        // proportional height is 2.5e-3dp; it must clamp up to MIN_THUMB.
        let max_scroll = 4_000_000.0 - 100.0;
        let got = metrics(0.0, max_scroll).expect("there is something to scroll");
        assert_eq!(got.thumb.size.height, MIN_THUMB);
    }

    #[test]
    fn the_minimum_thumb_never_exceeds_a_short_container() {
        // A 10dp-tall container cannot hold a 24dp thumb.
        let got = scrollbar_metrics(Point::ZERO, Size::new(200.0, 10.0), 0.0, 900.0)
            .expect("there is something to scroll");
        assert_eq!(got.thumb.size.height, 10.0, "the thumb is the lane");
        assert_eq!(got.thumb.origin.y, 0.0);
    }

    #[test]
    fn an_offset_past_the_limit_clamps_to_the_bottom() {
        // The model shrank while scrolled to the bottom: `scroll_y` can
        // exceed a freshly computed limit for one frame. The thumb must
        // stay inside its lane rather than sliding out.
        let got = metrics(500.0, 100.0).expect("there is something to scroll");
        assert_eq!(got.thumb.origin.y, 50.0);
        assert_eq!(got.thumb.origin.y + got.thumb.size.height, 100.0);
    }

    #[test]
    fn a_negative_offset_clamps_to_the_top() {
        let got = metrics(-40.0, 100.0).expect("there is something to scroll");
        assert_eq!(got.thumb.origin.y, 0.0);
    }

    #[test]
    fn the_thumb_hugs_the_right_edge_inside_the_inset() {
        let got = scrollbar_metrics(Point::new(30.0, 40.0), Size::new(200.0, 100.0), 0.0, 100.0)
            .expect("there is something to scroll");
        let right = got.thumb.origin.x + got.thumb.size.width;
        assert_eq!(right, 30.0 + 200.0 - THUMB_INSET);
        assert_eq!(got.thumb.size.width, THUMB_WIDTH);
    }

    #[test]
    fn the_geometry_follows_the_container_origin() {
        // Nested inside a translated ancestor: everything shifts together.
        let at_origin = metrics(25.0, 100.0).expect("scrollable");
        let offset =
            scrollbar_metrics(Point::new(30.0, 40.0), Size::new(200.0, 100.0), 25.0, 100.0)
                .expect("scrollable");
        assert_eq!(offset.thumb.origin.y, at_origin.thumb.origin.y + 40.0);
        assert_eq!(offset.track.origin, Point::new(30.0, 40.0));
    }

    #[test]
    fn a_thumb_is_never_taller_than_its_lane() {
        // Sweep the ratio space: the invariant must hold everywhere, not
        // just at the cases above.
        for viewport_height in [1.0_f32, 10.0, 24.0, 25.0, 100.0, 1000.0] {
            for max_scroll in [0.5_f32, 1.0, 50.0, 500.0, 1e6] {
                let got = scrollbar_metrics(
                    Point::ZERO,
                    Size::new(200.0, viewport_height),
                    0.0,
                    max_scroll,
                )
                .expect("a positive limit is always scrollable");
                assert!(
                    got.thumb.size.height <= viewport_height,
                    "thumb {} taller than lane {viewport_height}",
                    got.thumb.size.height
                );
                assert!(
                    got.thumb.size.height > 0.0,
                    "thumb must be visible: {}",
                    got.thumb.size.height
                );
                let bottom = got.thumb.origin.y + got.thumb.size.height;
                assert!(
                    bottom <= viewport_height + f32::EPSILON,
                    "thumb bottom {bottom} past lane {viewport_height}"
                );
            }
        }
    }

    /// The 100dp lane used by the drag tests. The limit is passed at each
    /// call site because some of these tests change it mid-gesture.
    fn lane() -> Size {
        return Size::new(200.0, 100.0);
    }

    #[test]
    fn dragging_the_thumb_to_the_top_lands_on_zero() {
        let got = scroll_y_from_thumb_top(lane(), 0.0, 100.0);
        assert_eq!(got, Some(0.0));
    }

    #[test]
    fn dragging_the_thumb_to_the_bottom_lands_exactly_on_the_limit() {
        // The whole point of sharing the travel with the painter: at the
        // bottom of the lane the answer must be the limit *exactly*, not
        // the limit minus an epsilon.
        let travel = 100.0 - 50.0;
        let got = scroll_y_from_thumb_top(lane(), travel, 100.0);
        assert_eq!(got, Some(100.0));
    }

    #[test]
    fn the_middle_of_the_lane_is_the_middle_of_the_range() {
        let travel = 100.0 - 50.0;
        let got = scroll_y_from_thumb_top(lane(), travel / 2.0, 100.0);
        assert_eq!(got, Some(50.0));
    }

    #[test]
    fn a_thumb_dragged_past_either_end_clamps() {
        assert_eq!(scroll_y_from_thumb_top(lane(), -30.0, 100.0), Some(0.0));
        assert_eq!(scroll_y_from_thumb_top(lane(), 500.0, 100.0), Some(100.0));
    }

    #[test]
    fn a_container_with_no_scrollbar_has_nothing_to_drag() {
        // Mirrors `scrollbar_metrics`: if no thumb is drawn, no thumb can
        // be grabbed. Guards against a drag that only *looks* dead.
        assert_eq!(scroll_y_from_thumb_top(lane(), 0.0, 0.0), None);
        assert_eq!(
            scroll_y_from_thumb_top(Size::new(200.0, 0.0), 0.0, 100.0),
            None
        );
    }

    #[test]
    fn a_lane_whose_thumb_fills_it_reports_no_travel() {
        // A 10dp container: the thumb is clamped to the whole lane, so it
        // has nowhere to move. There *is* a scrollbar (the content
        // overflows) but dragging it cannot change anything — it reads as
        // "the start" wherever it is dropped. Returning `None` instead
        // would make the bar vanish, which is the bug this pins.
        let short = Size::new(200.0, 10.0);
        assert!(scrollbar_metrics(Point::ZERO, short, 0.0, 900.0).is_some());
        // Any drop position maps to the top, and nothing divides by zero.
        assert_eq!(scroll_y_from_thumb_top(short, 0.0, 900.0), Some(0.0));
        assert_eq!(scroll_y_from_thumb_top(short, 7.0, 900.0), Some(0.0));
    }

    #[test]
    fn the_drag_mapping_round_trips_the_painter_placement() {
        // The property that makes the two directions safe to keep apart:
        // for any offset, drawing the thumb and then reading its position
        // back returns the offset. This is what "the bar follows the mouse"
        // means numerically.
        for viewport_height in [30.0_f32, 100.0, 400.0] {
            for max_scroll in [1.0_f32, 50.0, 1000.0, 1e5] {
                let size = Size::new(200.0, viewport_height);
                for step in 0..=10 {
                    let scroll_y = max_scroll * (step as f32) / 10.0;
                    let metrics = scrollbar_metrics(Point::ZERO, size, scroll_y, max_scroll)
                        .expect("a positive limit is scrollable");
                    let back = scroll_y_from_thumb_top(size, metrics.thumb.origin.y, max_scroll)
                        .expect("the same container is draggable");
                    assert!(
                        (back - scroll_y).abs() < 0.01,
                        "{scroll_y} -> thumb at {} -> {back} (lane {viewport_height})",
                        metrics.thumb.origin.y
                    );
                }
            }
        }
    }

    #[test]
    fn a_pointer_over_the_thumb_reads_as_a_thumb_hit() {
        let got = scrollbar_hit(Point::ZERO, lane(), 0.0, 100.0, Point::new(198.0, 10.0));
        assert_eq!(got, Some(ScrollbarHit::Thumb));
    }

    #[test]
    fn a_pointer_over_the_lane_but_off_the_thumb_reads_as_the_track() {
        // Thumb is at the top (0..50); y = 80 is lane, not thumb.
        let got = scrollbar_hit(Point::ZERO, lane(), 0.0, 100.0, Point::new(198.0, 80.0));
        assert_eq!(got, Some(ScrollbarHit::Track));
    }

    #[test]
    fn a_pointer_off_the_bar_reads_as_no_hit() {
        // Left of the thumb by more than the slop.
        assert_eq!(
            scrollbar_hit(Point::ZERO, lane(), 0.0, 100.0, Point::new(180.0, 10.0)),
            None
        );
        // Above or below the container entirely.
        assert_eq!(
            scrollbar_hit(Point::ZERO, lane(), 0.0, 100.0, Point::new(198.0, -5.0)),
            None
        );
        assert_eq!(
            scrollbar_hit(Point::ZERO, lane(), 0.0, 100.0, Point::new(198.0, 105.0)),
            None
        );
    }

    #[test]
    fn the_hit_strip_reaches_left_of_the_thumb_by_the_slop() {
        // Thumb left edge is at 200 - 2 - 4 = 194. The slop makes the strip
        // start at 188; one dp further left is the content's.
        assert!(scrollbar_hit(Point::ZERO, lane(), 0.0, 100.0, Point::new(189.0, 5.0)).is_some());
        assert_eq!(
            scrollbar_hit(Point::ZERO, lane(), 0.0, 100.0, Point::new(187.0, 5.0)),
            None
        );
    }

    #[test]
    fn a_container_with_nothing_to_scroll_has_nothing_to_grab() {
        // No thumb drawn means no hit at all, even squarely on the strip.
        // Otherwise a click on a short list's right edge would be swallowed
        // by a bar that is not there.
        assert_eq!(
            scrollbar_hit(Point::ZERO, lane(), 0.0, 0.0, Point::new(198.0, 10.0)),
            None
        );
    }
}
