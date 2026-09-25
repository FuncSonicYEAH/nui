//! SVG path data parsing and flattening (FUTURE batch 2).
//!
//! [`parse_path`] parses the `d` attribute grammar subset
//! `M/L/H/V/C/S/Q/T/Z` (absolute and relative; the `A` elliptical-arc
//! command is deferred) into absolute commands; [`flatten`] converts the
//! commands into polygon loops ready for triangulation and stroking.
//! Quadratic segments are elevated to cubics so one flattening routine
//! covers both.

use crate::geometry::Point;

/// One absolute path command (relative forms are resolved at parse time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCommand {
    /// Begins a new subpath at `at`.
    MoveTo(Point),
    /// Straight line to `to`.
    LineTo(Point),
    /// Cubic bezier with control points `c1`/`c2`, ending at `to`.
    CubicTo { c1: Point, c2: Point, to: Point },
    /// Quadratic bezier with control point `ctrl`, ending at `to`.
    QuadraticTo { ctrl: Point, to: Point },
    /// Closes the current subpath back to its start.
    Close,
}

/// Why a `d` string failed to parse (rendering silently skips such paths).
#[derive(Debug, Clone, PartialEq)]
pub struct PathError {
    /// Byte offset into the `d` string where the problem sits.
    pub position: usize,
    /// What went wrong.
    pub message: String,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return write!(
            formatter,
            "path error at byte {}: {}",
            self.position, self.message
        );
    }
}

impl std::error::Error for PathError {}

/// Which command letter the parser last handled; the `bool` marks the
/// lowercase (relative) form.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Command {
    Move(bool),
    Line(bool),
    Horizontal(bool),
    Vertical(bool),
    Cubic(bool),
    SmoothCubic(bool),
    Quadratic(bool),
    SmoothQuadratic(bool),
    Close,
}

impl Command {
    /// Whether this is a relative (lowercase) command.
    fn is_relative(self) -> bool {
        return match self {
            Command::Move(relative)
            | Command::Line(relative)
            | Command::Horizontal(relative)
            | Command::Vertical(relative)
            | Command::Cubic(relative)
            | Command::SmoothCubic(relative)
            | Command::Quadratic(relative)
            | Command::SmoothQuadratic(relative) => relative,
            Command::Close => false,
        };
    }

    /// Whether this command can take parameter pairs (and so repeats
    /// implicitly when a bare number follows).
    fn repeatable(self) -> bool {
        return !matches!(self, Command::Close);
    }
}

/// Parses a `d` attribute into absolute [`PathCommand`]s.
///
/// Whitespace and commas act as separators anywhere between tokens; the
/// number after a command may omit separators entirely (`M0,0L48,0`), and
/// repeated parameters repeat the last command (SVG's implicit-line part
/// of `M` is covered by that rule).
pub fn parse_path(d: &str) -> Result<Vec<PathCommand>, PathError> {
    let mut parser = Parser {
        bytes: d.as_bytes(),
        position: 0,
    };
    let mut commands = Vec::new();
    let mut current = Point::ZERO;
    let mut subpath_start = Point::ZERO;
    let mut previous = Option::<Command>::None;
    let mut first = true;
    while parser.skip_separators() {
        let kind = parser.next_command_or_repeat(previous, first)?;
        // Reflection sources for S/T are judged from the PREVIOUS command:
        // after C/S a smooth cubic reflects the prior c2; after Q/T a
        // smooth quadratic reflects the prior ctrl; anything else makes the
        // current point its own reflection.
        let smooth_cubic_source = matches!(
            previous,
            Some(Command::Cubic(_)) | Some(Command::SmoothCubic(_))
        );
        let smooth_quadratic_source = matches!(
            previous,
            Some(Command::Quadratic(_)) | Some(Command::SmoothQuadratic(_))
        );
        previous = Some(kind);
        first = false;
        match kind {
            Command::Move(_) => {
                let x = parser.number("move")?;
                let y = parser.number("move")?;
                current = offset(kind, current, x, y);
                subpath_start = current;
                commands.push(PathCommand::MoveTo(current));
            }
            Command::Line(_) => {
                let x = parser.number("line")?;
                let y = parser.number("line")?;
                current = offset(kind, current, x, y);
                commands.push(PathCommand::LineTo(current));
            }
            Command::Horizontal(relative) => {
                let x = parser.number("horizontal line")?;
                current = if relative {
                    Point::new(current.x + x, current.y)
                } else {
                    Point::new(x, current.y)
                };
                commands.push(PathCommand::LineTo(current));
            }
            Command::Vertical(relative) => {
                let y = parser.number("vertical line")?;
                current = if relative {
                    Point::new(current.x, current.y + y)
                } else {
                    Point::new(current.x, y)
                };
                commands.push(PathCommand::LineTo(current));
            }
            Command::Cubic(_) => {
                let c1 = offset(kind, current, parser.number("cubic")?, parser.number("cubic")?);
                let c2 = offset(kind, current, parser.number("cubic")?, parser.number("cubic")?);
                let to = offset(kind, current, parser.number("cubic")?, parser.number("cubic")?);
                current = to;
                commands.push(PathCommand::CubicTo { c1, c2, to });
            }
            Command::SmoothCubic(_) => {
                // S: the first control point reflects the previous C/S c2
                // about `current`.
                let c1 = reflect(last_c2_of(&commands, smooth_cubic_source, current), current);
                let c2 = offset(kind, current, parser.number("cubic")?, parser.number("cubic")?);
                let to = offset(kind, current, parser.number("cubic")?, parser.number("cubic")?);
                current = to;
                commands.push(PathCommand::CubicTo { c1, c2, to });
            }
            Command::Quadratic(_) => {
                let ctrl = offset(kind, current, parser.number("quadratic")?, parser.number("quadratic")?);
                let to = offset(kind, current, parser.number("quadratic")?, parser.number("quadratic")?);
                current = to;
                commands.push(PathCommand::QuadraticTo { ctrl, to });
            }
            Command::SmoothQuadratic(_) => {
                let ctrl = reflect(
                    last_ctrl_of(&commands, smooth_quadratic_source, current),
                    current,
                );
                let to = offset(kind, current, parser.number("quadratic")?, parser.number("quadratic")?);
                current = to;
                commands.push(PathCommand::QuadraticTo { ctrl, to });
            }
            Command::Close => {
                commands.push(PathCommand::Close);
                current = subpath_start;
            }
        }
    }
    return Ok(commands);
}

/// Applies an absolute/relative pair `(x, y)` against `current`.
fn offset(kind: Command, current: Point, x: f32, y: f32) -> Point {
    if kind.is_relative() {
        return Point::new(current.x + x, current.y + y);
    }
    return Point::new(x, y);
}

/// The second control point of the last cubic in `commands` (for S
/// reflection), or `current` when the previous command was not a cubic.
fn last_c2_of(commands: &[PathCommand], previous_was_cubic: bool, current: Point) -> Point {
    if !previous_was_cubic {
        return current;
    }
    return match commands.last() {
        Some(PathCommand::CubicTo { c2, .. }) => *c2,
        _ => current,
    };
}

/// The control point of the last quadratic in `commands` (for T
/// reflection), or `current` when the previous command was not a quadratic.
fn last_ctrl_of(commands: &[PathCommand], previous_was_quadratic: bool, current: Point) -> Point {
    if !previous_was_quadratic {
        return current;
    }
    return match commands.last() {
        Some(PathCommand::QuadraticTo { ctrl, .. }) => *ctrl,
        _ => current,
    };
}

/// Flattens absolute path commands into polygon loops (one per subpath),
/// subdividing curves until their control points sit within `tolerance`
/// (dp) of the chord. `Z` physically closes a loop by repeating its start
/// point; an unterminated final subpath comes out open (still fine to
/// stroke, and `earcut` ignores rings with fewer than 3 points).
pub fn flatten(commands: &[PathCommand], tolerance: f32) -> Vec<Vec<Point>> {
    let tolerance = tolerance.max(0.01);
    let mut loops: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut subpath_start = Point::ZERO;
    let mut last = Point::ZERO;
    for command in commands {
        match *command {
            PathCommand::MoveTo(at) => {
                if current.len() >= 2 {
                    loops.push(std::mem::take(&mut current));
                }
                current.clear();
                current.push(at);
                subpath_start = at;
                last = at;
            }
            PathCommand::LineTo(to) => {
                current.push(to);
                last = to;
            }
            PathCommand::CubicTo { c1, c2, to } => {
                flatten_cubic(last, c1, c2, to, tolerance, &mut current);
                last = to;
            }
            PathCommand::QuadraticTo { ctrl, to } => {
                // Elevate to a cubic (exact representation) and reuse the
                // cubic flattener.
                let c1 = Point::new(
                    last.x + (ctrl.x - last.x) * 2.0 / 3.0,
                    last.y + (ctrl.y - last.y) * 2.0 / 3.0,
                );
                let c2 = Point::new(
                    to.x + (ctrl.x - to.x) * 2.0 / 3.0,
                    to.y + (ctrl.y - to.y) * 2.0 / 3.0,
                );
                flatten_cubic(last, c1, c2, to, tolerance, &mut current);
                last = to;
            }
            PathCommand::Close => {
                if current.len() >= 3 {
                    if let Some(start) = subpath_start_of(&current) {
                        current.push(start);
                    }
                    loops.push(std::mem::take(&mut current));
                }
                current.clear();
                last = subpath_start;
            }
        }
    }
    if current.len() >= 2 {
        loops.push(current);
    }
    return loops;
}

/// The start point of the subpath in `current` (the first point after the
/// leading MoveTo).
fn subpath_start_of(current: &[Point]) -> Option<Point> {
    return current.first().copied();
}

/// Bisects a cubic bezier until flat (de Casteljau subdivision with an
/// explicit stack — no recursion-depth surprises on extreme input).
fn flatten_cubic(p0: Point, p1: Point, p2: Point, p3: Point, tolerance: f32, out: &mut Vec<Point>) {
    // Each stack entry carries one segment's four control points.
    let mut stack = vec![(p0, p1, p2, p3)];
    while let Some((a, b, c, d)) = stack.pop() {
        // Flat when both inner control points sit within `tolerance` of the
        // chord a-d.
        let flat =
            distance_to_chord(b, a, d) <= tolerance && distance_to_chord(c, a, d) <= tolerance;
        if flat {
            out.push(d);
            continue;
        }
        // de Casteljau at t = 0.5.
        let ab = mid(a, b);
        let bc = mid(b, c);
        let cd = mid(c, d);
        let abc = mid(ab, bc);
        let bcd = mid(bc, cd);
        let abcd = mid(abc, bcd);
        stack.push((abcd, bcd, cd, d));
        stack.push((a, ab, abc, abcd));
    }
}

/// Distance from `p` to the chord line through `a` and `b` (the flattener
/// judges control points, and the recursion converges regardless of the
/// chord's extent, so the infinite-line form is sufficient here).
fn distance_to_chord(p: Point, a: Point, b: Point) -> f32 {
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let length2 = vx * vx + vy * vy;
    if length2 < 1e-12 {
        return (p.x - a.x).hypot(p.y - a.y);
    }
    let cross = (p.x - a.x) * vy - (p.y - a.y) * vx;
    return cross.abs() / length2.sqrt();
}

/// The midpoint of two points.
fn mid(a: Point, b: Point) -> Point {
    return Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
}

/// `p` reflected about `center`.
fn reflect(p: Point, center: Point) -> Point {
    return Point::new(center.x * 2.0 - p.x, center.y * 2.0 - p.y);
}

/// Character scanner over the `d` string.
struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Parser<'_> {
    /// Skips whitespace and commas (SVG allows them interchangeably between
    /// tokens). Returns whether input remains.
    fn skip_separators(&mut self) -> bool {
        while let Some(&byte) = self.bytes.get(self.position) {
            if byte.is_ascii_whitespace() || byte == b',' {
                self.position += 1;
            } else {
                break;
            }
        }
        return self.position < self.bytes.len();
    }

    /// Reads the next command letter, or repeats the previous command when
    /// a bare number appears where a letter was expected (implicit
    /// repetition — and per the SVG spec, a repeated `M` continues with
    /// `L`/`l` lines).
    fn next_command_or_repeat(
        &mut self,
        previous: Option<Command>,
        first: bool,
    ) -> Result<Command, PathError> {
        let byte = self.bytes[self.position];
        let kind = match byte {
            b'M' => Command::Move(false),
            b'm' => Command::Move(true),
            b'L' => Command::Line(false),
            b'l' => Command::Line(true),
            b'H' => Command::Horizontal(false),
            b'h' => Command::Horizontal(true),
            b'V' => Command::Vertical(false),
            b'v' => Command::Vertical(true),
            b'C' => Command::Cubic(false),
            b'c' => Command::Cubic(true),
            b'S' => Command::SmoothCubic(false),
            b's' => Command::SmoothCubic(true),
            b'Q' => Command::Quadratic(false),
            b'q' => Command::Quadratic(true),
            b'T' => Command::SmoothQuadratic(false),
            b't' => Command::SmoothQuadratic(true),
            b'Z' | b'z' => Command::Close,
            b'A' | b'a' => {
                return Err(self.error(
                    "elliptical arc (A) is not supported yet; use curves or polylines",
                ));
            }
            _ => {
                if Self::starts_number(byte) {
                    return match previous {
                        // The SVG rule that extra M pairs continue as lines.
                        Some(Command::Move(relative)) => Ok(Command::Line(relative)),
                        Some(kind) if kind.repeatable() => Ok(kind),
                        _ => Err(self.error("number without a repeatable command")),
                    };
                }
                return Err(self.error(format!("unexpected character `{}`", byte as char)));
            }
        };
        if first && !matches!(kind, Command::Move(_)) {
            return Err(self.error("path data must start with a move (M or m)"));
        }
        self.position += 1;
        return Ok(kind);
    }

    /// Reads one number (SVG float grammar: sign, digits, optional decimal
    /// part, optional exponent). Separators (including commas) are allowed
    /// before it; a sign acts as its own separator (`1-2` is two numbers).
    fn number(&mut self, context: &str) -> Result<f32, PathError> {
        self.skip_separators();
        let start = self.position;
        let mut end = start;
        if matches!(self.bytes.get(end), Some(b'+') | Some(b'-')) {
            end += 1;
        }
        let mut digits = 0;
        while matches!(self.bytes.get(end), Some(byte) if byte.is_ascii_digit()) {
            end += 1;
            digits += 1;
        }
        if matches!(self.bytes.get(end), Some(b'.')) {
            end += 1;
            while matches!(self.bytes.get(end), Some(byte) if byte.is_ascii_digit()) {
                end += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return Err(self.error(format!("expected a number for {context}")));
        }
        // Exponent part (only when digits follow the marker, so `2ex`
        // parses as the number 2 plus garbage later, not as an error here).
        if matches!(self.bytes.get(end), Some(b'e') | Some(b'E')) {
            let mut lookahead = end + 1;
            if matches!(self.bytes.get(lookahead), Some(b'+') | Some(b'-')) {
                lookahead += 1;
            }
            if matches!(self.bytes.get(lookahead), Some(byte) if byte.is_ascii_digit()) {
                while matches!(self.bytes.get(lookahead), Some(byte) if byte.is_ascii_digit()) {
                    lookahead += 1;
                }
                end = lookahead;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..end]).unwrap_or("0");
        self.position = end;
        return text
            .parse::<f32>()
            .map_err(|_| return self.error(format!("unparseable number for {context}")));
    }

    /// Whether a byte can begin a number.
    fn starts_number(byte: u8) -> bool {
        return byte.is_ascii_digit() || byte == b'.' || byte == b'+' || byte == b'-';
    }

    /// Builds an error at the current position.
    fn error(&self, message: impl Into<String>) -> PathError {
        return PathError {
            position: self.position,
            message: message.into(),
        };
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    /// A tiny helper so test expectations read like the grammar.
    fn move_to(x: f32, y: f32) -> PathCommand {
        return PathCommand::MoveTo(Point::new(x, y));
    }

    fn line_to(x: f32, y: f32) -> PathCommand {
        return PathCommand::LineTo(Point::new(x, y));
    }

    #[test]
    fn parses_absolute_moveto_lineto_close() {
        let commands = parse_path("M 0 0 L 48 0 L 24 42 Z").expect("parses");
        assert_eq!(
            commands,
            vec![
                move_to(0.0, 0.0),
                line_to(48.0, 0.0),
                line_to(24.0, 42.0),
                PathCommand::Close,
            ]
        );
    }

    #[test]
    fn parses_compact_syntax_without_separators() {
        let commands = parse_path("M0,0L48,0L24,42z").expect("parses");
        assert_eq!(commands.len(), 4);
        assert_eq!(commands[3], PathCommand::Close);
    }

    #[test]
    fn resolves_relative_commands_against_the_current_point() {
        let commands = parse_path("M 10 10 l 20 0 h 5 v -8 l -10 4").expect("parses");
        assert_eq!(
            commands,
            vec![
                move_to(10.0, 10.0),
                line_to(30.0, 10.0),
                line_to(35.0, 10.0),
                line_to(35.0, 2.0),
                line_to(25.0, 6.0),
            ]
        );
        // H/V with negatives, floats, and dot-led numbers.
        let mixed = parse_path("M1.5 2.5 H-3 V.5").expect("parses");
        assert_eq!(
            mixed,
            vec![move_to(1.5, 2.5), line_to(-3.0, 2.5), line_to(-3.0, 0.5)]
        );
    }

    #[test]
    fn moveto_extra_pairs_repeat_as_lineto() {
        let commands = parse_path("M 0 0 10 10 20 0").expect("parses");
        assert_eq!(
            commands,
            vec![move_to(0.0, 0.0), line_to(10.0, 10.0), line_to(20.0, 0.0)]
        );
    }

    #[test]
    fn cubic_and_quadratic_store_absolute_points() {
        let commands = parse_path("M 0 0 C 5 5 10 5 15 0 S 25 -5 30 0 Q 35 10 40 0 T 50 0")
            .expect("parses");
        assert_eq!(
            commands[1],
            PathCommand::CubicTo {
                c1: Point::new(5.0, 5.0),
                c2: Point::new(10.0, 5.0),
                to: Point::new(15.0, 0.0),
            }
        );
        // S reflects the previous c2 (10,5) about current (15,0) -> (20,-5).
        assert_eq!(
            commands[2],
            PathCommand::CubicTo {
                c1: Point::new(20.0, -5.0),
                c2: Point::new(25.0, -5.0),
                to: Point::new(30.0, 0.0),
            }
        );
        assert_eq!(
            commands[3],
            PathCommand::QuadraticTo {
                ctrl: Point::new(35.0, 10.0),
                to: Point::new(40.0, 0.0),
            }
        );
        // T reflects the previous ctrl (35,10) about current (40,0) -> (45,-10).
        assert_eq!(
            commands[4],
            PathCommand::QuadraticTo {
                ctrl: Point::new(45.0, -10.0),
                to: Point::new(50.0, 0.0),
            }
        );
    }

    #[test]
    fn smooth_after_non_matching_kind_reflects_the_current_point() {
        // S right after M: c1 degenerates to the current point.
        let commands = parse_path("M 5 5 S 10 0 15 5").expect("parses");
        assert_eq!(
            commands[1],
            PathCommand::CubicTo {
                c1: Point::new(5.0, 5.0),
                c2: Point::new(10.0, 0.0),
                to: Point::new(15.0, 5.0),
            }
        );
    }

    #[test]
    fn close_returns_the_pen_to_the_subpath_start() {
        // After Z the pen sits at the subpath start, so a following l is
        // relative to (0,0), not to the pre-close position.
        let commands = parse_path("M 0 0 L 10 0 Z l 5 5").expect("parses");
        assert_eq!(commands[3], line_to(5.0, 5.0));
    }

    #[test]
    fn rejects_broken_input() {
        // Not starting with a move.
        assert!(parse_path("L 0 0").is_err());
        // Unknown/unsupported command.
        assert!(parse_path("M 0 0 A 5 5 0 0 1 10 10").is_err());
        assert!(parse_path("M 0 0 X 5").is_err());
        // Missing parameters.
        assert!(parse_path("M 0 0 L 10").is_err());
        assert!(parse_path("M 0 0 C 1 1 2 2").is_err());
        // A bare number with no command behind it.
        assert!(parse_path("42").is_err());
        assert!(parse_path("M 0 0 Z 5").is_err(), "close takes no parameters");
    }

    #[test]
    fn empty_input_and_stray_separators_are_tolerated() {
        assert_eq!(parse_path("").expect("empty is ok"), Vec::new());
        assert!(parse_path("  , ").expect("separators only") .is_empty());
        assert!(parse_path("M 0 0 ,,,,").is_ok(), "stray separators are tolerated");
    }

    #[test]
    fn scientific_notation_and_negative_signs_parse() {
        let commands = parse_path("M1e1 2.5e0L-3.5-.25").expect("parses");
        assert_eq!(commands[0], move_to(10.0, 2.5));
        assert_eq!(commands[1], line_to(-3.5, -0.25));
    }

    #[test]
    fn flatten_keeps_straight_subpaths_exactly() {
        let commands = parse_path("M 0 0 L 48 0 L 24 42 Z").expect("parses");
        let loops = flatten(&commands, 0.1);
        assert_eq!(loops.len(), 1);
        // Lines add no intermediate points; Z duplicates the start point.
        assert_eq!(
            loops[0],
            vec![
                Point::new(0.0, 0.0),
                Point::new(48.0, 0.0),
                Point::new(24.0, 42.0),
                Point::new(0.0, 0.0),
            ]
        );
    }

    #[test]
    fn flatten_subdivides_curves_within_tolerance() {
        // A quarter circle approximated by a cubic (kappa form): the circle
        // has center (0, 100) and radius 100, running from (0,0) to
        // (100,100). The control points sit far from the chord, so the
        // flattener must subdivide beyond the single endpoint.
        let commands = parse_path("M 0 0 C 55.2 0 100 44.8 100 100").expect("parses");
        let coarse = flatten(&commands, 5.0);
        let fine = flatten(&commands, 0.1);
        assert!(coarse[0].len() >= 4, "coarse still subdivides to hit the arc");
        assert!(fine[0].len() > coarse[0].len(), "smaller tolerance subdivides more");
        // Every sampled point lies close to the true quarter arc.
        for point in &fine[0] {
            let radius = (point.x - 0.0).hypot(point.y - 100.0);
            assert!(
                (radius - 100.0).abs() < 1.0,
                "on the arc within 1dp, got {point:?}"
            );
        }
    }

    #[test]
    fn flatten_produces_one_loop_per_subpath() {
        let commands = parse_path("M 0 0 L 10 0 L 10 10 Z M 20 0 L 30 0 L 30 10 Z").expect("parses");
        let loops = flatten(&commands, 0.1);
        assert_eq!(loops.len(), 2);
        assert_eq!(loops[0][0], Point::new(0.0, 0.0));
        assert_eq!(loops[1][0], Point::new(20.0, 0.0));
    }

    #[test]
    fn flatten_drops_single_point_subpaths_but_keeps_open_two_point_ones() {
        let commands = parse_path("M 5 5 M 0 0 L 10 0").expect("parses");
        let loops = flatten(&commands, 0.1);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0], vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)]);
    }

    #[test]
    fn close_on_a_degenerate_subpath_produces_nothing() {
        // "M 5 5 Z" closes a one-point subpath: no ring comes out, and the
        // pen returns to the subpath start either way.
        let commands = parse_path("M 5 5 Z l 5 5").expect("parses");
        assert!(flatten(&commands, 0.1).is_empty());
        assert_eq!(commands[2], line_to(10.0, 10.0));
    }
}
