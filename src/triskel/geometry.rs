//! Small, shared geometry predicates used by routing and layout validation.
//!
//! `GEOMETRY_EPS` shrinks rectangles before testing.  Values within that
//! tolerance of a face are boundary contact, not an interior hit; non-finite
//! input is never considered an intersection.

use crate::triskel::layout::Point;

pub(crate) const GEOMETRY_EPS: f64 = 1e-9;

/// Returns whether the finite segment `a..b` enters the strict interior of the
/// axis-aligned rectangle centred at `center` with the supplied dimensions.
/// Face/corner tangency is harmless.  A zero-length segment hits only when its
/// point is strictly inside.
pub(crate) fn segment_enters_rect_strict(
    a: Point,
    b: Point,
    center: Point,
    width: f64,
    height: f64,
) -> bool {
    if ![a.x, a.y, b.x, b.y, center.x, center.y, width, height]
        .into_iter()
        .all(f64::is_finite)
        || width <= 2.0 * GEOMETRY_EPS
        || height <= 2.0 * GEOMETRY_EPS
    {
        return false;
    }
    // Test against the epsilon-shrunk *open* rectangle.  Maintaining an open
    // parameter interval makes tangencies false without relying on NaNs or a
    // floating-point equality accident.
    let xmin = center.x - width / 2.0 + GEOMETRY_EPS;
    let xmax = center.x + width / 2.0 - GEOMETRY_EPS;
    let ymin = center.y - height / 2.0 + GEOMETRY_EPS;
    let ymax = center.y + height / 2.0 - GEOMETRY_EPS;
    let mut lo: f64 = 0.0;
    let mut hi: f64 = 1.0;
    for (p, d, min, max) in [(a.x, b.x - a.x, xmin, xmax), (a.y, b.y - a.y, ymin, ymax)] {
        if d.abs() <= GEOMETRY_EPS {
            if p <= min || p >= max {
                return false;
            }
            continue;
        }
        let t0 = (min - p) / d;
        let t1 = (max - p) / d;
        lo = lo.max(t0.min(t1));
        hi = hi.min(t0.max(t1));
    }
    lo < hi - GEOMETRY_EPS && hi > 0.0 && lo < 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn strict_segment_rectangle_intersections_are_exact() {
        let cases = [
            (p(-2., 0.), p(2., 0.), true, "centre"),
            (p(-2., 0.99), p(2., 0.99), true, "near top edge"),
            (p(-2., -0.99), p(2., -0.99), true, "near bottom edge"),
            (p(-2., 1.), p(2., 1.), false, "top tangent"),
            (p(-2., -1.), p(2., -1.), false, "bottom tangent"),
            (p(1., -2.), p(1., 2.), false, "right tangent"),
            (p(-1., -2.), p(-1., 2.), false, "left tangent"),
            (p(-2., -2.), p(-1., -1.), false, "corner tangent"),
            (p(0., 0.), p(0.5, 0.5), true, "wholly inside"),
            (p(-2., 1.5), p(2., 1.5), false, "overlapping bbox only"),
            (p(0., -2.), p(0., 2.), true, "vertical"),
            (p(-2., 0.), p(2., 0.), true, "horizontal"),
            (p(0., 0.), p(0., 0.), true, "point inside"),
            (p(1., 0.), p(1., 0.), false, "point on boundary"),
            (p(2., 2.), p(2., 2.), false, "point outside"),
        ];
        for (a, b, expected, name) in cases {
            assert_eq!(
                segment_enters_rect_strict(a, b, p(0., 0.), 2., 2.),
                expected,
                "{name}"
            );
            assert_eq!(
                segment_enters_rect_strict(b, a, p(0., 0.), 2., 2.),
                expected,
                "reversed {name}"
            );
        }
    }
}
