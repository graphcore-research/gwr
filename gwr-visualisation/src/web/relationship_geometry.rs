// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Point {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

pub(crate) fn interpolate_hierarchy<const N: usize>(
    hierarchy: [Point; N],
    strength: f64,
) -> [Point; N] {
    let start = hierarchy[0];
    let end = hierarchy[N - 1];
    std::array::from_fn(|index| {
        let point = hierarchy[index];
        let progress = index as f64 / (N - 1) as f64;
        let line = Point {
            x: start.x + (end.x - start.x) * progress,
            y: start.y + (end.y - start.y) * progress,
        };
        Point {
            x: line.x + (point.x - line.x) * strength,
            y: line.y + (point.y - line.y) * strength,
        }
    })
}

pub(crate) fn bezier_controls(
    previous: Point,
    current: Point,
    next: Point,
    following: Point,
) -> (Point, Point) {
    (
        Point {
            x: current.x + (next.x - previous.x) / 6.0,
            y: current.y + (next.y - previous.y) / 6.0,
        },
        Point {
            x: next.x - (following.x - current.x) / 6.0,
            y: next.y - (following.y - current.y) / 6.0,
        },
    )
}

pub(crate) fn edge_alpha(edge_count: usize, weight: f64) -> f64 {
    let density = (edge_count as f64 / 250.0).min(1.0);
    0.28 - density * 0.18 + weight * (0.5 - density * 0.25)
}

#[cfg(test)]
mod tests {
    use super::{Point, edge_alpha, interpolate_hierarchy};

    #[test]
    fn zero_strength_flattens_the_hierarchy_to_a_line() {
        let points = interpolate_hierarchy(
            [
                Point { x: 0.0, y: 0.0 },
                Point { x: 5.0, y: 8.0 },
                Point { x: 10.0, y: 0.0 },
            ],
            0.0,
        );

        assert_eq!(points[1], Point { x: 5.0, y: 0.0 });
    }

    #[test]
    fn full_strength_preserves_hierarchy_points() {
        let hierarchy = [
            Point { x: 0.0, y: 0.0 },
            Point { x: 5.0, y: 8.0 },
            Point { x: 10.0, y: 0.0 },
        ];

        assert_eq!(interpolate_hierarchy(hierarchy, 1.0), hierarchy);
    }

    #[test]
    fn dense_graphs_use_lower_alpha() {
        assert!(edge_alpha(500, 0.5) < edge_alpha(10, 0.5));
    }
}
