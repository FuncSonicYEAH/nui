//! Geometry primitives: point, size, and rectangle (dp coordinate space).

/// 2D point. Plain data type with public fields for direct access.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Horizontal coordinate (dp, positive is right).
    pub x: f32,
    /// Vertical coordinate (dp, positive is down, screen coordinates).
    pub y: f32,
}

impl Point {
    /// Origin (0, 0).
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    /// Constructs a point.
    pub const fn new(x: f32, y: f32) -> Point {
        return Point { x, y };
    }
}

/// 2D size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    /// Width (dp).
    pub width: f32,
    /// Height (dp).
    pub height: f32,
}

impl Size {
    /// Zero size.
    pub const ZERO: Size = Size {
        width: 0.0,
        height: 0.0,
    };

    /// Constructs a size.
    pub const fn new(width: f32, height: f32) -> Size {
        return Size { width, height };
    }

    /// A size is empty when either side is non-positive (no visible area).
    pub fn is_empty(&self) -> bool {
        return self.width <= 0.0 || self.height <= 0.0;
    }
}

/// Axis-aligned rectangle defined by top-left `origin` and `size`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Top-left corner (dp).
    pub origin: Point,
    /// Size (dp).
    pub size: Size,
}

impl Rect {
    /// Zero rectangle.
    pub const ZERO: Rect = Rect {
        origin: Point::ZERO,
        size: Size::ZERO,
    };

    /// Constructs a rectangle.
    pub const fn new(origin: Point, size: Size) -> Rect {
        return Rect { origin, size };
    }

    /// Right edge x coordinate (origin.x + width).
    pub const fn right(&self) -> f32 {
        return self.origin.x + self.size.width;
    }

    /// Bottom edge y coordinate (origin.y + height).
    pub const fn bottom(&self) -> f32 {
        return self.origin.y + self.size.height;
    }

    /// Whether the rectangle is empty (see [`Size::is_empty`]).
    pub fn is_empty(&self) -> bool {
        return self.size.is_empty();
    }

    /// Whether the point lies inside the rectangle. Top-left inclusive,
    /// bottom-right exclusive (pixel-coverage semantics, so adjacent
    /// rectangles never overlap).
    pub fn contains(&self, point: Point) -> bool {
        return point.x >= self.origin.x
            && point.y >= self.origin.y
            && point.x < self.right()
            && point.y < self.bottom();
    }

    /// Shrinks by `margin` on all sides (negative values expand outward).
    pub fn inset(&self, margin: f32) -> Rect {
        return Rect::new(
            Point::new(self.origin.x + margin, self.origin.y + margin),
            Size::new(
                self.size.width - margin - margin,
                self.size.height - margin - margin,
            ),
        );
    }

    /// Smallest rectangle covering both; empty rectangles act as the union
    /// identity and are ignored.
    pub fn union(&self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return *self;
        }
        let left = self.origin.x.min(other.origin.x);
        let top = self.origin.y.min(other.origin.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        return Rect::new(Point::new(left, top), Size::new(right - left, bottom - top));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_is_left_top_closed_right_bottom_open() {
        let rect = Rect::new(Point::new(10.0, 10.0), Size::new(100.0, 100.0));
        assert!(rect.contains(Point::new(10.0, 10.0)));
        assert!(rect.contains(Point::new(109.9, 109.9)));
        assert!(!rect.contains(Point::new(110.0, 50.0)));
        assert!(!rect.contains(Point::new(50.0, 110.0)));
        assert!(!rect.contains(Point::new(9.9, 50.0)));
    }

    #[test]
    fn inset_shrinks_uniformly_and_negative_expands() {
        let rect = Rect::new(Point::ZERO, Size::new(100.0, 50.0));
        let inner = rect.inset(5.0);
        assert_eq!(
            inner,
            Rect::new(Point::new(5.0, 5.0), Size::new(90.0, 40.0))
        );
        let outer = rect.inset(-5.0);
        assert_eq!(
            outer,
            Rect::new(Point::new(-5.0, -5.0), Size::new(110.0, 60.0))
        );
    }

    #[test]
    fn union_covers_both_and_ignores_empty() {
        let first = Rect::new(Point::ZERO, Size::new(10.0, 10.0));
        let second = Rect::new(Point::new(20.0, 20.0), Size::new(10.0, 10.0));
        let covered = Rect::new(Point::ZERO, Size::new(30.0, 30.0));
        assert_eq!(first.union(second), covered);
        assert_eq!(Rect::ZERO.union(second), second);
        assert_eq!(first.union(Rect::ZERO), first);
    }

    #[test]
    fn empty_size_makes_rect_empty() {
        let rect = Rect::new(Point::ZERO, Size::new(0.0, 10.0));
        assert!(rect.is_empty());
        assert!(!rect.contains(Point::ZERO));
    }

    #[test]
    fn edges_report_far_sides() {
        let rect = Rect::new(Point::new(1.0, 2.0), Size::new(10.0, 20.0));
        assert_eq!(rect.right(), 11.0);
        assert_eq!(rect.bottom(), 22.0);
    }
}
