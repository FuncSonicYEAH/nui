//! Ear-clipping triangulation for simple polygons (FUTURE batch 2).
//!
//! The minimal base version from the geometry plan: one ring, no holes,
//! no even-odd filling — good enough for UI shapes (icons, charts), with
//! the pixel tests as the correctness net. Holes and robustness upgrades
//! warrant a real library later.

use crate::geometry::Point;

/// Triangulates one ring into triangles indexing `points` (ring-local
/// indices, each triple wound the same way as the input). Returns an empty
/// vec for degenerate input (fewer than 3 points, all collinear, or
/// duplicate-only rings).
pub fn triangulate(points: &[Point]) -> Vec<[u32; 3]> {
    let mut ring: Vec<u32> = clean_ring(points);
    if ring.len() < 3 {
        return Vec::new();
    }
    // Signed area decides winding; ear clipping works on counter-clockwise
    // rings (positive area in screen coordinates where y grows down), so a
    // clockwise input is reversed first.
    if signed_area(points) < 0.0 {
        ring.reverse();
    }
    let mut triangles = Vec::new();
    while ring.len() > 2 {
        // When no ear is clipable the remaining ring is degenerate
        // (collapsed or self-touching); stop instead of spinning forever.
        let Some((previous, current, next)) = find_ear(&ring, points) else {
            break;
        };
        triangles.push([ring[previous], ring[current], ring[next]]);
        ring.remove(current);
    }
    return triangles;
}

/// Signed area of the ring (shoelace formula). Positive = counter-clockwise
/// in a y-down coordinate system.
fn signed_area(points: &[Point]) -> f32 {
    let mut area = 0.0;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        area += a.x * b.y - b.x * a.y;
    }
    return area * 0.5;
}

/// Drops duplicate closing points, consecutive duplicates, and collinear
/// vertices, and returns ring vertex indices into `points`.
fn clean_ring(points: &[Point]) -> Vec<u32> {
    let mut ring: Vec<u32> = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        // Skip exact duplicates of the previous kept point.
        if let Some(&last) = ring.last()
            && points[last as usize] == *point
        {
            continue;
        }
        ring.push(index as u32);
    }
    // Drop a duplicate closing vertex.
    if let Some(&closing) = ring.last()
        && ring.len() > 1
        && points[ring[0] as usize] == points[closing as usize]
    {
        ring.pop();
    }
    // Remove collinear vertices (they form zero-area ears).
    let mut deduplicated = Vec::with_capacity(ring.len());
    for (position, &index) in ring.iter().enumerate() {
        let previous = ring[(position + ring.len() - 1) % ring.len()] as usize;
        let next = ring[(position + 1) % ring.len()] as usize;
        if cross(points[previous], points[index as usize], points[next]) != 0.0 {
            deduplicated.push(index);
        }
    }
    return deduplicated;
}

/// Twice the signed area of triangle (a, b, c); the sign gives the turn
/// direction (positive = counter-clockwise in y-down screen coordinates).
fn cross(a: Point, b: Point, c: Point) -> f32 {
    return (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
}

/// Finds one clipable ear: ring position triple (previous, current, next)
/// where the corner is convex and no other ring vertex intrudes into the
/// ear triangle.
fn find_ear(ring: &[u32], points: &[Point]) -> Option<(usize, usize, usize)> {
    let count = ring.len();
    for current in 0..count {
        let previous = (current + count - 1) % count;
        let next = (current + 1) % count;
        let a = points[ring[previous] as usize];
        let b = points[ring[current] as usize];
        let c = points[ring[next] as usize];
        // Convex corner only (a reflex corner cannot be an ear tip).
        if cross(a, b, c) <= 0.0 {
            continue;
        }
        if ring_is_clear_of(ring, points, &[a, b, c], &[previous, current, next]) {
            return Some((previous, current, next));
        }
    }
    return None;
}

/// Whether no *other* ring vertex lies inside (or on the border of) the ear
/// triangle `corner`. Vertices equal to the corner's own points are skipped
/// by comparing ring positions in `skip`.
fn ring_is_clear_of(
    ring: &[u32],
    points: &[Point],
    corner: &[Point; 3],
    skip: &[usize; 3],
) -> bool {
    for (position, &index) in ring.iter().enumerate() {
        if skip.contains(&position) {
            continue;
        }
        let vertex = points[index as usize];
        // Coincident vertices do not intrude; strictly inside or on an edge
        // does (on-edge coincidences of distinct indices stay excluded so
        // duplicated coordinates cannot swallow an ear).
        if point_in_triangle_inclusive(vertex, corner) {
            let coincident = vertex == corner[0] || vertex == corner[1] || vertex == corner[2];
            if !coincident {
                return false;
            }
        }
    }
    return true;
}

/// Whether `p` lies inside the triangle `t`, border included (with a small
/// epsilon so on-edge vertices reliably block the ear).
fn point_in_triangle_inclusive(p: Point, t: &[Point; 3]) -> bool {
    let d1 = cross(t[0], t[1], p);
    let d2 = cross(t[1], t[2], p);
    let d3 = cross(t[2], t[0], p);
    let epsilon = -1e-9;
    return d1 >= epsilon && d2 >= epsilon && d3 >= epsilon;
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> Point {
        return Point::new(x, y);
    }

    /// Total triangle area, to compare against the polygon's shoelace area.
    fn triangle_area(triangles: &[[u32; 3]], points: &[Point]) -> f32 {
        let mut area = 0.0;
        for [a, b, c] in triangles {
            let area_tri = (cross(points[*a as usize], points[*b as usize], points[*c as usize])).abs() * 0.5;
            area += area_tri;
        }
        return area;
    }

    #[test]
    fn square_yields_two_triangles_covering_the_area() {
        let square = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        let triangles = triangulate(&square);
        assert_eq!(triangles.len(), 2);
        assert!((triangle_area(&triangles, &square) - 100.0).abs() < 1e-4);
        // All indices are valid and each triangle reuses the input points.
        for [a, b, c] in &triangles {
            assert!(*a < 4 && *b < 4 && *c < 4);
        }
    }

    #[test]
    fn clockwise_rings_triangulate_the_same_as_ccw() {
        let ccw = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        let mut cw = ccw;
        cw.reverse();
        let a = triangulate(&ccw);
        let b = triangulate(&cw);
        assert_eq!(a.len(), b.len());
        assert!((triangle_area(&a, &ccw) - triangle_area(&b, &cw)).abs() < 1e-4);
    }

    #[test]
    fn concave_star_triangulates_without_flipping() {
        // A 5-pointed star: the classic concave case where naive fan
        // triangulation fails. Ten vertices around (20, 20).
        let mut star = Vec::new();
        for step in 0..10 {
            let angle = std::f32::consts::TAU * step as f32 / 10.0 - std::f32::consts::FRAC_PI_2;
            let radius = if step % 2 == 0 { 20.0 } else { 8.0 };
            star.push(p(20.0 + radius * angle.cos(), 20.0 + radius * angle.sin()));
        }
        let triangles = triangulate(&star);
        // A simple polygon of n vertices yields exactly n - 2 triangles.
        assert_eq!(triangles.len(), 8);
        let expected: f32 = signed_area(&star);
        assert!((triangle_area(&triangles, &star) - expected.abs()).abs() < 1e-3);
    }

    #[test]
    fn collinear_and_duplicate_vertices_are_cleaned() {
        // Square with midpoints on each edge and a duplicated closing
        // vertex: still two triangles.
        let mut ring = vec![
            p(0.0, 0.0),
            p(5.0, 0.0),
            p(10.0, 0.0),
            p(10.0, 5.0),
            p(10.0, 10.0),
            p(5.0, 10.0),
            p(0.0, 10.0),
            p(0.0, 5.0),
        ];
        ring.push(p(0.0, 0.0));
        let triangles = triangulate(&ring);
        assert_eq!(triangles.len(), 2);
    }

    #[test]
    fn degenerate_rings_produce_nothing() {
        assert!(triangulate(&[]).is_empty());
        assert!(triangulate(&[p(0.0, 0.0)]).is_empty());
        assert!(triangulate(&[p(0.0, 0.0), p(10.0, 10.0)]).is_empty());
        // All collinear.
        assert!(triangulate(&[p(0.0, 0.0), p(5.0, 5.0), p(10.0, 10.0)]).is_empty());
        // Zero area: same point repeated.
        assert!(triangulate(&[p(3.0, 3.0), p(3.0, 3.0), p(3.0, 3.0)]).is_empty());
    }
}
