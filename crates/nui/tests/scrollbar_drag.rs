//! Dragging a scrollbar, end to end through its public halves.
//!
//! `WindowHost` cannot be built headless (it needs a window and a GPU), so
//! the host's pointer sequence is replicated here from the pieces it calls:
//! `element_bounds` → `scrollbar_hit` / `scrollbar_metrics` →
//! `scroll_y_from_thumb_top` → `set_direct("scroll_y")`. If the host's
//! drag path changes shape, this file must follow — that is the price of
//! driving the seam without a display, and the same bargain
//! `tests/gallery_interaction.rs` already strikes for clicks and typing.
//!
//! What this file is *for*: the unit tests in
//! `nui-render/src/widget/scrollbar.rs` pin the arithmetic, and the host
//! has no tests at all. Neither can catch a bar that is drawn in one place
//! and grabbed in another, or a drag that follows the pointer in the wrong
//! direction. Those are the bugs measured here, against a tree that has
//! actually been laid out.

#![allow(clippy::unwrap_used)]

use nui_core::{Color, Point, Size, Value};
use nui_render::widget::{ScrollbarHit, scroll_y_from_thumb_top, scrollbar_hit, scrollbar_metrics};
use nui_runtime::{Element, ElementId, ElementTree, Engine};

/// The scroll container's box, and the content inside it.
const CONTAINER: Size = Size {
    width: 200.0,
    height: 100.0,
};
const CONTENT_HEIGHT: f32 = 400.0;

/// A laid-out `Scroll` holding one tall child: `CONTENT_HEIGHT` dp of
/// content in a `CONTAINER`-tall viewport, so there are 300dp to travel.
fn scrollable() -> (ElementTree, Engine, ElementId) {
    let engine = Engine::new();
    let mut tree = ElementTree::new();
    let mut scroll = Element::new("Scroll", None);
    scroll.set("x", Value::Float(0.0));
    scroll.set("y", Value::Float(0.0));
    scroll.set("width", Value::Float(f64::from(CONTAINER.width)));
    scroll.set("height", Value::Float(f64::from(CONTAINER.height)));
    scroll.set("fill", Value::Color(Color::from_rgb8(20, 20, 20)));
    let scroll_id = tree.insert(scroll);
    tree.push_root(scroll_id);
    let mut child = Element::new("Rectangle", None);
    child.set("x", Value::Float(0.0));
    child.set("y", Value::Float(0.0));
    child.set("width", Value::Float(f64::from(CONTAINER.width)));
    child.set("height", Value::Float(f64::from(CONTENT_HEIGHT)));
    child.set("fill", Value::Color(Color::from_rgb8(200, 40, 40)));
    let child_id = tree.insert(child);
    tree.append_child(scroll_id, child_id);
    return (tree, engine, scroll_id);
}

/// The pointer's y over the middle of the thumb, for a container scrolled
/// to `scroll_y`.
fn thumb_centre_y(engine: &Engine, tree: &ElementTree, container: ElementId, scroll_y: f32) -> f32 {
    let bounds = bounds_of(tree, container);
    let limit = nui_runtime::widget::max_scroll_y(engine, tree, container);
    let metrics = scrollbar_metrics(bounds.origin, bounds.size, scroll_y, limit)
        .expect("a container with overflow has a scrollbar");
    return metrics.thumb.origin.y + metrics.thumb.size.height / 2.0;
}

/// An x inside the bar's grab strip: the centre of where the thumb is
/// drawn, which is `right - THUMB_INSET - THUMB_WIDTH/2`.
///
/// Not "one dp inside the container's right edge": the thumb is inset from
/// that edge, so the last few dp of the container are *past* the bar.
fn bar_x(tree: &ElementTree, container: ElementId) -> f32 {
    let bounds = bounds_of(tree, container);
    return bounds.origin.x + bounds.size.width
        - nui_render::widget::THUMB_INSET
        - nui_render::widget::THUMB_WIDTH / 2.0;
}

fn bounds_of(tree: &ElementTree, id: ElementId) -> nui_core::Rect {
    return nui::element_bounds(tree, id).expect("a laid-out element has a box");
}

/// One frame of the host's *press* path: ask what the bar is under
/// `position`, and if the thumb is grabbed, record the grab offset.
///
/// Mirrors `WindowHost::begin_scrollbar_drag`. Returns the offset to pass
/// to [`drag_to`], or `None` when the press was not on the thumb (a track
/// click, or not on the bar at all).
fn press_thumb(
    engine: &Engine,
    tree: &ElementTree,
    container: ElementId,
    position: Point,
) -> Option<f32> {
    let bounds = bounds_of(tree, container);
    let limit = nui_runtime::widget::max_scroll_y(engine, tree, container);
    let scroll_y = scroll_y_of(tree, container);
    let metrics = scrollbar_metrics(bounds.origin, bounds.size, scroll_y, limit)?;
    return match scrollbar_hit(bounds.origin, bounds.size, scroll_y, limit, position)? {
        ScrollbarHit::Thumb => Some(position.y - metrics.thumb.origin.y),
        ScrollbarHit::Track => None,
    };
}

/// One frame of the host's *drag* path: write the `scroll_y` the pointer
/// stands for, given the grab offset recorded at press time.
///
/// Mirrors `WindowHost::drag_scrollbar_to`, and deliberately does **no**
/// hit test: a live drag owns the pointer wherever it goes, so a drag that
/// leaves the container keeps scrolling. Testing the press's hit test again
/// here would make the test stricter than the host and hide exactly the
/// bug this file exists to catch.
fn drag_to(
    engine: &mut Engine,
    tree: &mut ElementTree,
    container: ElementId,
    position: Point,
    grab_offset: f32,
) -> bool {
    let bounds = bounds_of(tree, container);
    let limit = nui_runtime::widget::max_scroll_y(engine, tree, container);
    let scroll_y = scroll_y_of(tree, container);
    let Some(metrics) = scrollbar_metrics(bounds.origin, bounds.size, scroll_y, limit) else {
        return false;
    };
    let lane_offset = (position.y - grab_offset) - metrics.track.origin.y;
    let Some(next) = scroll_y_from_thumb_top(metrics.track.size, lane_offset, limit) else {
        return false;
    };
    return engine.set_direct(tree, container, "scroll_y", Value::Float(f64::from(next)));
}

/// A press on the track, then a drag: what the host does when the press
/// lands on the lane rather than the thumb.
fn click_track(
    engine: &mut Engine,
    tree: &mut ElementTree,
    container: ElementId,
    position: Point,
) -> bool {
    // The track jump puts the thumb's top at the pointer, i.e. a grab
    // offset of zero.
    return drag_to(engine, tree, container, position, 0.0);
}

fn scroll_y_of(tree: &ElementTree, container: ElementId) -> f32 {
    return match tree.arena[container].get("scroll_y") {
        Some(Value::Float(value)) => *value as f32,
        _ => 0.0,
    };
}

fn limit_of(engine: &Engine, tree: &ElementTree, container: ElementId) -> f32 {
    return nui_runtime::widget::max_scroll_y(engine, tree, container);
}

#[test]
fn the_container_reports_room_to_scroll() {
    let (tree, engine, id) = scrollable();
    assert_eq!(
        limit_of(&engine, &tree, id),
        CONTENT_HEIGHT - CONTAINER.height
    );
}

#[test]
fn pressing_the_thumb_grabs_it() {
    let (tree, engine, id) = scrollable();
    let bounds = bounds_of(&tree, id);
    let hit = scrollbar_hit(
        bounds.origin,
        bounds.size,
        0.0,
        limit_of(&engine, &tree, id),
        Point::new(bar_x(&tree, id), thumb_centre_y(&engine, &tree, id, 0.0)),
    );
    assert_eq!(hit, Some(ScrollbarHit::Thumb));
}

#[test]
fn pressing_elsewhere_in_the_container_is_not_a_scrollbar_hit() {
    // The bar lives in the right-hand few dp; the bulk of the container is
    // the content's.
    let (tree, engine, id) = scrollable();
    let bounds = bounds_of(&tree, id);
    let limit = limit_of(&engine, &tree, id);
    assert_eq!(
        scrollbar_hit(
            bounds.origin,
            bounds.size,
            0.0,
            limit,
            Point::new(bounds.origin.x + 20.0, 50.0)
        ),
        None
    );
    // And a container with nothing to scroll has no bar to press at all,
    // even squarely on the strip.
    assert_eq!(
        scrollbar_hit(
            bounds.origin,
            bounds.size,
            0.0,
            0.0,
            Point::new(bar_x(&tree, id), 50.0)
        ),
        None
    );
}

#[test]
fn dragging_the_thumb_down_scrolls_the_content_down() {
    let (mut tree, mut engine, id) = scrollable();
    let x = bar_x(&tree, id);
    // Grab the thumb at its centre (a press the bar reports as the thumb),
    // then move 40dp down the lane.
    let grab_point = Point::new(x, thumb_centre_y(&engine, &tree, id, 0.0));
    let grab_offset =
        press_thumb(&engine, &tree, id, grab_point).expect("the press is on the thumb");
    assert!(
        grab_offset > 0.0,
        "grabbing below the thumb's top edge: {grab_offset}"
    );

    let moved = drag_to(
        &mut engine,
        &mut tree,
        id,
        Point::new(x, grab_point.y + 40.0),
        grab_offset,
    );
    assert!(moved, "the drag wrote a new offset");
    assert!(
        scroll_y_of(&tree, id) > 0.0,
        "dragging the thumb down scrolls the content down: got {}",
        scroll_y_of(&tree, id)
    );
}

#[test]
fn the_drag_follows_the_pointer_monotonically() {
    // Sweep the pointer down the lane; the offset must never go backwards,
    // which is the visible symptom of a mapping that mixes up its ends.
    let (mut tree, mut engine, id) = scrollable();
    let x = bar_x(&tree, id);
    let bounds = bounds_of(&tree, id);
    let metrics = scrollbar_metrics(
        bounds.origin,
        bounds.size,
        0.0,
        limit_of(&engine, &tree, id),
    )
    .unwrap();
    let grab_offset = metrics.thumb.size.height / 2.0;
    let start = metrics.thumb.origin.y + grab_offset;

    let mut previous = -1.0_f32;
    for step in 0..=10 {
        let y = start + (step as f32 / 10.0) * (CONTAINER.height - start);
        drag_to(&mut engine, &mut tree, id, Point::new(x, y), grab_offset);
        let now = scroll_y_of(&tree, id);
        assert!(
            now >= previous - 0.01,
            "step {step}: {previous} -> {now} went backwards"
        );
        previous = now;
    }
    assert!(
        previous > 0.0,
        "the sweep actually moved the content: {previous}"
    );
}

#[test]
fn dragging_to_the_bottom_of_the_lane_reaches_the_limit_exactly() {
    // The end that matters: if the drag stops a hair short, the last row
    // never comes into view and nothing says why.
    let (mut tree, mut engine, id) = scrollable();
    let x = bar_x(&tree, id);
    let limit = limit_of(&engine, &tree, id);
    let bottom = {
        let bounds = bounds_of(&tree, id);
        let metrics = scrollbar_metrics(bounds.origin, bounds.size, 0.0, limit).unwrap();
        // Put the grab at the thumb's centre and drive the pointer to the
        // lane's bottom edge.
        metrics.thumb.size.height / 2.0
    };
    drag_to(
        &mut engine,
        &mut tree,
        id,
        Point::new(x, CONTAINER.height),
        bottom,
    );
    assert_eq!(scroll_y_of(&tree, id), limit);
}

#[test]
fn a_track_click_jumps_towards_the_pointer() {
    // Not a drag: a click below the thumb puts the thumb's top at the
    // pointer, so content scrolls to roughly there.
    let (mut tree, mut engine, id) = scrollable();
    let x = bar_x(&tree, id);
    let y = CONTAINER.height - 10.0;
    // The bar's own hit test says this is the track, not the thumb, which
    // is what makes it a jump rather than a grab.
    assert_eq!(
        scrollbar_hit(
            bounds_of(&tree, id).origin,
            bounds_of(&tree, id).size,
            0.0,
            limit_of(&engine, &tree, id),
            Point::new(x, y)
        ),
        Some(ScrollbarHit::Track)
    );
    assert!(
        press_thumb(&engine, &tree, id, Point::new(x, y)).is_none(),
        "a track press records no grab offset"
    );
    let wrote = click_track(&mut engine, &mut tree, id, Point::new(x, y));
    assert!(wrote, "the click wrote an offset");
    let now = scroll_y_of(&tree, id);
    assert!(now > 0.0, "content moved: {now}");
    assert!(
        now <= limit_of(&engine, &tree, id),
        "and stayed within the limit: {now}"
    );
}

/// Whether the host's *press* path consumes a press at `position`.
///
/// Mirrors `WindowHost::begin_scrollbar_drag`'s return value: `true` means
/// the press belonged to the bar and must not reach the content underneath.
/// A track click jumps; a thumb press opens a drag; either way the bar
/// takes the gesture.
fn press_consumed_by_bar(
    engine: &mut Engine,
    tree: &mut ElementTree,
    container: ElementId,
    position: Point,
) -> bool {
    let bounds = bounds_of(tree, container);
    let limit = nui_runtime::widget::max_scroll_y(engine, tree, container);
    let scroll_y = scroll_y_of(tree, container);
    let Some(hit) = scrollbar_hit(bounds.origin, bounds.size, scroll_y, limit, position) else {
        return false;
    };
    match hit {
        ScrollbarHit::Thumb => return true,
        ScrollbarHit::Track => {
            // The jump runs, and the press is consumed even when the jump
            // writes the offset that was already there.
            drag_to(engine, tree, container, position, 0.0);
            return true;
        }
    }
}

#[test]
fn a_press_on_the_bar_is_always_consumed() {
    // Regression: a track click whose jump happened to land on the current
    // offset used to report "unhandled" and fall through to the content,
    // arming whatever button was behind the bar.
    let (mut tree, mut engine, id) = scrollable();
    let x = bar_x(&tree, id);
    // A point at the very top of the lane: the thumb is already there, so
    // the jump changes nothing.
    let on_thumb = Point::new(x, 2.0);
    assert!(
        press_consumed_by_bar(&mut engine, &mut tree, id, on_thumb),
        "a press on the bar belongs to the bar"
    );
    // And a track press, likewise, whatever it writes.
    let (mut tree, mut engine, id) = scrollable();
    let x = bar_x(&tree, id);
    let on_track = Point::new(x, CONTAINER.height - 5.0);
    assert!(
        press_consumed_by_bar(&mut engine, &mut tree, id, on_track),
        "a press on the track belongs to the bar too"
    );
}

#[test]
fn a_press_on_the_content_is_not_consumed_by_the_bar() {
    let (mut tree, mut engine, id) = scrollable();
    let bounds = bounds_of(&tree, id);
    let in_content = Point::new(bounds.origin.x + 30.0, bounds.origin.y + 30.0);
    assert!(
        !press_consumed_by_bar(&mut engine, &mut tree, id, in_content),
        "the body of the container is the content's, not the bar's"
    );
}

#[test]
fn a_drag_outside_the_container_still_scrolls() {
    // Pointer capture is the point of the gesture: the bar keeps following
    // the cursor even when it leaves the container's box, so a drag that
    // runs off the bottom does not lose the thumb.
    let (mut tree, mut engine, id) = scrollable();
    let x = bar_x(&tree, id);
    let grab_offset = 10.0;
    let wrote = drag_to(
        &mut engine,
        &mut tree,
        id,
        Point::new(x, CONTAINER.height + 80.0),
        grab_offset,
    );
    assert!(wrote, "a press that left the box is still the same gesture");
    assert_eq!(
        scroll_y_of(&tree, id),
        limit_of(&engine, &tree, id),
        "and clamps at the bottom rather than running past it"
    );
}

#[test]
fn a_container_with_no_overflow_cannot_be_dragged() {
    // Content shorter than the viewport: no bar, so no hit, so no drag.
    // Without this guard a click on a short list's right edge would write a
    // scroll position to a container that has none to give.
    let mut engine = Engine::new();
    let mut tree = ElementTree::new();
    let mut scroll = Element::new("Scroll", None);
    scroll.set("x", Value::Float(0.0));
    scroll.set("y", Value::Float(0.0));
    scroll.set("width", Value::Float(f64::from(CONTAINER.width)));
    scroll.set("height", Value::Float(f64::from(CONTAINER.height)));
    let id = tree.insert(scroll);
    tree.push_root(id);
    let mut child = Element::new("Rectangle", None);
    child.set("x", Value::Float(0.0));
    child.set("y", Value::Float(0.0));
    child.set("width", Value::Float(f64::from(CONTAINER.width)));
    child.set("height", Value::Float(50.0));
    let child_id = tree.insert(child);
    tree.append_child(id, child_id);

    let bounds = bounds_of(&tree, id);
    assert_eq!(limit_of(&engine, &tree, id), 0.0);
    assert_eq!(
        scrollbar_hit(
            bounds.origin,
            bounds.size,
            0.0,
            0.0,
            Point::new(bar_x(&tree, id), 50.0)
        ),
        None
    );
    let x = bar_x(&tree, id);
    // The host's press gate: no thumb to grab.
    assert!(
        press_thumb(&engine, &tree, id, Point::new(x, 50.0)).is_none(),
        "a container with no overflow has no thumb to press"
    );
    // And even if a drag were somehow opened, there is no limit to map
    // against, so nothing is written.
    let wrote = drag_to(&mut engine, &mut tree, id, Point::new(x, 50.0), 0.0);
    assert!(!wrote, "nothing to drag");
    assert_eq!(scroll_y_of(&tree, id), 0.0);
}

#[test]
fn the_bar_the_user_grabs_is_the_bar_that_is_drawn() {
    // The invariant behind the whole feature: the hit strip and the painted
    // thumb come from one geometry. Assert the strip contains the thumb
    // everywhere, at several scroll positions, so no rounding can separate
    // them.
    let (tree, engine, id) = scrollable();
    let bounds = bounds_of(&tree, id);
    let limit = limit_of(&engine, &tree, id);
    for step in 0..=8 {
        let scroll_y = limit * (step as f32 / 8.0);
        let metrics = scrollbar_metrics(bounds.origin, bounds.size, scroll_y, limit).unwrap();
        // Sample the thumb's top, middle and bottom edges.
        for y in [
            metrics.thumb.origin.y,
            metrics.thumb.origin.y + metrics.thumb.size.height / 2.0,
            metrics.thumb.origin.y + metrics.thumb.size.height,
        ] {
            let hit = scrollbar_hit(
                bounds.origin,
                bounds.size,
                scroll_y,
                limit,
                Point::new(metrics.thumb.origin.x + 1.0, y),
            );
            assert_eq!(
                hit,
                Some(ScrollbarHit::Thumb),
                "scroll_y {scroll_y}: the drawn thumb at y={y} is not grabbable"
            );
        }
    }
}
