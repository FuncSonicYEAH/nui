//! Canvas command buffer (FUTURE batch 3): the imperative escape hatch.
//!
//! A `Canvas` element is only a shell in `.nui`; the host registers a
//! behavior whose [`CanvasPainter`] records [`CanvasOp`]s into a shared
//! buffer. The scene builder interprets the buffer per frame — curves
//! flatten into rings, fills go through ear clipping, strokes reuse the
//! capsule pipeline — and the result composites through the offscreen
//! layer pipeline, so Canvas adds zero new GPU code.

use std::cell::RefCell;
use std::rc::Rc;

use nui_core::Color;

/// Stroke end caps for canvas stroking (mirrors nui-render's LineCap;
/// the render crate depends on this one, not the other way around).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasCap {
    /// Square ends exactly at the endpoints.
    Butt,
    /// Semicircular caps extending half the stroke width past the endpoints.
    Round,
}

/// One recorded drawing command (canvas-local dp coordinates, y down).
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasOp {
    /// Begins a new subpath at `(x, y)`.
    MoveTo { x: f32, y: f32 },
    /// Appends a straight segment to `(x, y)`.
    LineTo { x: f32, y: f32 },
    /// Appends a cubic bezier to `(x, y)`.
    CubicTo {
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    },
    /// Appends a quadratic bezier to `(x, y)`.
    QuadraticTo { cx: f32, cy: f32, x: f32, y: f32 },
    /// Closes the current subpath back to its start.
    Close,
    /// Fills every subpath recorded so far (fill rules follow the simple
    /// earcut base: one non-self-intersecting loop per subpath is v1).
    Fill { color: Color },
    /// Strokes every subpath recorded so far with the capsule pipeline.
    Stroke {
        width: f32,
        cap: CanvasCap,
        color: Color,
    },
    /// Drops all recorded subpaths (the buffer of commands stays; only the
    /// interpreter's path state resets).
    Clear,
}

/// The host-facing painter: a cloneable handle over the shared command
/// buffer. Commands append in order; the scene builder interprets the
/// whole buffer each frame (v1 repaints every frame — see FUTURE batch 3).
#[derive(Clone, Default)]
pub struct CanvasPainter {
    ops: Rc<RefCell<Vec<CanvasOp>>>,
}

impl std::fmt::Debug for CanvasPainter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return formatter
            .debug_struct("CanvasPainter")
            .field("ops", &self.ops.borrow().len())
            .finish();
    }
}

impl CanvasPainter {
    /// Creates an empty painter.
    pub fn new() -> CanvasPainter {
        return CanvasPainter::default();
    }

    /// Begins a new subpath at `(x, y)`.
    pub fn move_to(&self, x: f32, y: f32) {
        self.ops.borrow_mut().push(CanvasOp::MoveTo { x, y });
    }

    /// Appends a straight segment to `(x, y)`.
    pub fn line_to(&self, x: f32, y: f32) {
        self.ops.borrow_mut().push(CanvasOp::LineTo { x, y });
    }

    /// Appends a cubic bezier to `(x, y)`.
    pub fn cubic_to(&self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.ops
            .borrow_mut()
            .push(CanvasOp::CubicTo { c1x, c1y, c2x, c2y, x, y });
    }

    /// Appends a quadratic bezier to `(x, y)`.
    pub fn quadratic_to(&self, cx: f32, cy: f32, x: f32, y: f32) {
        self.ops.borrow_mut().push(CanvasOp::QuadraticTo { cx, cy, x, y });
    }

    /// Closes the current subpath back to its start.
    pub fn close(&self) {
        self.ops.borrow_mut().push(CanvasOp::Close);
    }

    /// Fills every subpath recorded so far.
    pub fn fill(&self, color: Color) {
        self.ops.borrow_mut().push(CanvasOp::Fill { color });
    }

    /// Strokes every subpath recorded so far.
    pub fn stroke(&self, width: f32, cap: CanvasCap, color: Color) {
        self.ops
            .borrow_mut()
            .push(CanvasOp::Stroke { width, cap, color });
    }

    /// Drops all recorded subpaths.
    pub fn clear(&self) {
        self.ops.borrow_mut().push(CanvasOp::Clear);
    }

    /// Snapshot of the recorded commands (for the scene builder).
    pub fn ops(&self) -> Vec<CanvasOp> {
        return self.ops.borrow().clone();
    }
}

/// Interpreter output: flattened fill rings and strokeable subpaths, ready
/// to become PathDraw / PolylineDraw.
#[derive(Debug, Default, PartialEq)]
pub struct CanvasFrame {
    /// Flattened rings to fill, in command order.
    pub fills: Vec<(nui_core::Color, Vec<Vec<nui_core::Point>>)>,
    /// Subpaths to stroke, in command order.
    pub strokes: Vec<(
        nui_core::Color,
        f32,
        CanvasCap,
        Vec<Vec<nui_core::Point>>,
    )>,
}

/// Interprets a command buffer into fill rings and stroke subpaths.
/// Curves flatten at `tolerance` dp. Follows HTML-canvas semantics:
/// `fill`/`stroke` consume a snapshot of the current subpaths without
/// clearing them; only [`CanvasOp::Clear`] resets path state.
pub fn interpret(ops: &[CanvasOp], tolerance: f32) -> CanvasFrame {
    use nui_core::Point;

    let mut frame = CanvasFrame::default();
    let mut subpaths: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut last = Point::ZERO;
    for op in ops {
        match *op {
            CanvasOp::MoveTo { x, y } => {
                if current.len() >= 2 {
                    subpaths.push(std::mem::take(&mut current));
                }
                current.clear();
                current.push(Point::new(x, y));
                last = Point::new(x, y);
            }
            CanvasOp::LineTo { x, y } => {
                current.push(Point::new(x, y));
                last = Point::new(x, y);
            }
            CanvasOp::CubicTo {
                c1x,
                c1y,
                c2x,
                c2y,
                x,
                y,
            } => {
                // `flatten` derives the curve start from a leading MoveTo,
                // so the segment is anchored at the current pen position.
                let flat = nui_core::path::flatten(
                    &[
                        nui_core::path::PathCommand::MoveTo(last),
                        nui_core::path::PathCommand::CubicTo {
                            c1: Point::new(c1x, c1y),
                            c2: Point::new(c2x, c2y),
                            to: Point::new(x, y),
                        },
                    ],
                    tolerance,
                );
                if let Some(points) = flat.first() {
                    // Skip the MoveTo anchor; the curve samples follow.
                    for point in &points[1.min(points.len())..] {
                        current.push(*point);
                    }
                }
                last = Point::new(x, y);
            }
            CanvasOp::QuadraticTo { cx, cy, x, y } => {
                let flat = nui_core::path::flatten(
                    &[
                        nui_core::path::PathCommand::MoveTo(last),
                        nui_core::path::PathCommand::QuadraticTo {
                            ctrl: Point::new(cx, cy),
                            to: Point::new(x, y),
                        },
                    ],
                    tolerance,
                );
                if let Some(points) = flat.first() {
                    for point in &points[1.min(points.len())..] {
                        current.push(*point);
                    }
                }
                last = Point::new(x, y);
            }
            CanvasOp::Close => {
                if let Some(start) = current.first().copied() {
                    current.push(start);
                    if current.len() >= 3 {
                        subpaths.push(std::mem::take(&mut current));
                        last = start;
                    }
                }
                current = Vec::new();
            }
            CanvasOp::Fill { color } => {
                let mut loops: Vec<Vec<Point>> = subpaths.clone();
                if current.len() >= 2 {
                    loops.push(current.clone());
                }
                if !loops.is_empty() {
                    frame.fills.push((color, loops));
                }
            }
            CanvasOp::Stroke {
                width,
                cap,
                color,
            } => {
                let mut loops: Vec<Vec<Point>> = subpaths.clone();
                if current.len() >= 2 {
                    loops.push(current.clone());
                }
                if !loops.is_empty() {
                    frame.strokes.push((color, width, cap, loops));
                }
            }
            CanvasOp::Clear => {
                subpaths.clear();
                current.clear();
            }
        }
    }
    if current.len() >= 2 {
        subpaths.push(current);
    }
    return frame;
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::Point;

    #[test]
    fn painter_records_commands_in_order() {
        let painter = CanvasPainter::new();
        painter.move_to(0.0, 0.0);
        painter.line_to(10.0, 0.0);
        painter.close();
        painter.fill(Color::from_rgb8(255, 0, 0));
        painter.stroke(2.0, CanvasCap::Round, Color::WHITE);
        assert_eq!(
            painter.ops(),
            vec![
                CanvasOp::MoveTo { x: 0.0, y: 0.0 },
                CanvasOp::LineTo { x: 10.0, y: 0.0 },
                CanvasOp::Close,
                CanvasOp::Fill {
                    color: Color::from_rgb8(255, 0, 0)
                },
                CanvasOp::Stroke {
                    width: 2.0,
                    cap: CanvasCap::Round,
                    color: Color::WHITE,
                },
            ]
        );
        // Clones share the same buffer; `clear` APPENDS a command (the
        // interpreter resets path state on replay), so the op list grows.
        let twin = painter.clone();
        twin.clear();
        assert_eq!(painter.ops().len(), 6);
        assert_eq!(painter.ops()[5], CanvasOp::Clear);
    }

    #[test]
    fn interpret_produces_one_closed_ring_for_fill_and_stroke() {
        let ops = vec![
            CanvasOp::MoveTo { x: 0.0, y: 0.0 },
            CanvasOp::LineTo { x: 10.0, y: 0.0 },
            CanvasOp::LineTo { x: 10.0, y: 10.0 },
            CanvasOp::Close,
            CanvasOp::Fill {
                color: Color::WHITE,
            },
            CanvasOp::Stroke {
                width: 2.0,
                cap: CanvasCap::Round,
                color: Color::BLACK,
            },
        ];
        let frame = interpret(&ops, 0.1);
        assert_eq!(frame.fills.len(), 1);
        assert_eq!(frame.fills[0].1.len(), 1);
        assert_eq!(frame.fills[0].1[0].len(), 4, "closed ring of 3 points");
        assert_eq!(frame.strokes.len(), 1);
        assert_eq!(frame.strokes[0].1, 2.0);
        assert_eq!(frame.strokes[0].2, CanvasCap::Round);
    }

    #[test]
    fn interpret_flattens_curves_into_the_subpath() {
        let ops = vec![
            CanvasOp::MoveTo { x: 0.0, y: 0.0 },
            CanvasOp::CubicTo {
                c1x: 0.0,
                c1y: 55.2,
                c2x: 44.8,
                c2y: 100.0,
                x: 100.0,
                y: 100.0,
            },
            CanvasOp::Stroke {
                width: 1.0,
                cap: CanvasCap::Butt,
                color: Color::BLACK,
            },
        ];
        let frame = interpret(&ops, 5.0);
        assert_eq!(frame.strokes.len(), 1);
        let points = &frame.strokes[0].3[0];
        // Start + subdivided curve samples (the coarse tolerance still
        // subdivides a quarter circle).
        assert!(points.len() >= 4);
        assert_eq!(points[0], Point::new(0.0, 0.0));
        assert_eq!(*points.last().unwrap(), Point::new(100.0, 100.0));
    }

    #[test]
    fn fill_and_stroke_do_not_clear_the_path_but_clear_does() {
        let ops = vec![
            CanvasOp::MoveTo { x: 0.0, y: 0.0 },
            CanvasOp::LineTo { x: 10.0, y: 0.0 },
            CanvasOp::LineTo { x: 10.0, y: 10.0 },
            CanvasOp::Close,
            CanvasOp::Fill {
                color: Color::WHITE,
            },
            CanvasOp::Clear,
            CanvasOp::Fill {
                color: Color::BLACK,
            },
        ];
        let frame = interpret(&ops, 0.1);
        assert_eq!(frame.fills.len(), 1, "the fill after Clear has no subpaths");
        assert_eq!(frame.fills[0].1.len(), 1, "the first fill sees the ring");
    }

    #[test]
    fn moveto_splits_subpaths() {
        let ops = vec![
            CanvasOp::MoveTo { x: 0.0, y: 0.0 },
            CanvasOp::LineTo { x: 10.0, y: 0.0 },
            CanvasOp::MoveTo { x: 20.0, y: 0.0 },
            CanvasOp::LineTo { x: 30.0, y: 0.0 },
            CanvasOp::Fill {
                color: Color::WHITE,
            },
        ];
        let frame = interpret(&ops, 0.1);
        // Two 2-point open subpaths arrive as separate loops; earcut drops
        // the zero-area rings downstream.
        assert_eq!(frame.fills.len(), 1);
        assert_eq!(frame.fills[0].1.len(), 2);
    }

    /// Demo behavior painting one red triangle at construction time.
    #[derive(Debug)]
    struct DemoCanvas {
        painter: CanvasPainter,
    }

    impl crate::registry::ElementBehavior for DemoCanvas {
        fn on_signal(
            &mut self,
            _context: &mut crate::registry::BehaviorContext<'_>,
            _element: crate::element::ElementId,
            _signal: &str,
        ) {
        }

        fn canvas(&self) -> Option<CanvasPainter> {
            return Some(self.painter.clone());
        }
    }

    #[test]
    fn instantiation_hands_the_behavior_painter_to_the_element() {
        let source = r#"
component Board {
    Window(id = root) {
        Canvas(id = board, width = 200dp, height = 120dp)
    }
}
"#;
        let outcome = nui_compiler::compile(source);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);

        use crate::registry::{ComponentDesc, Registry};
        let mut registry = Registry::new();
        registry.register_component(
            ComponentDesc {
                name: "Canvas".to_string(),
                properties: Vec::new(),
            },
            Some(Box::new(|| {
                let painter = CanvasPainter::new();
                painter.move_to(10.0, 10.0);
                painter.line_to(190.0, 10.0);
                painter.line_to(100.0, 110.0);
                painter.close();
                painter.fill(Color::from_rgb8(255, 0, 0));
                return Box::new(DemoCanvas { painter });
            })),
        );
        let instance = crate::instantiate::instantiate_with(&outcome.document, registry);
        // Walk the tree for the Canvas element.
        let mut canvas_id = None;
        for (id, element) in instance.tree.arena.iter() {
            if element.ty == "Canvas" {
                canvas_id = Some(id);
            }
        }
        let canvas_id = canvas_id.expect("canvas element instantiated");
        let element = &instance.tree.arena[canvas_id];
        let painter = element.canvas.as_ref().expect("painter handed over");
        assert_eq!(painter.ops().len(), 5, "the demo triangle's commands");
        assert!(element.behavior.is_some(), "behavior stays attached");
    }
}
