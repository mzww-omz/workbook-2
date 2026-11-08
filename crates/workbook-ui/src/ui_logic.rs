//! Pure view-state transitions for the new answer widgets.
//!
//! Keeping these out of the GPUI element builders makes mouse, drag, and
//! keyboard actions share one implementation and lets them be tested without a
//! display.

use std::collections::BTreeMap;

use workbook_domain::HotspotShape;

/// Moves `id` so it lands at `target` in the resulting list.
pub(crate) fn move_order(order: &mut Vec<String>, id: &str, target: usize) -> bool {
    let Some(from) = order.iter().position(|item| item == id) else {
        return false;
    };
    let value = order.remove(from);
    let target = target.min(order.len());
    order.insert(target, value);
    true
}

/// Assigns a left/right or target/label pair. A value can only appear once, so
/// assigning it elsewhere removes the previous pair.
pub(crate) fn assign_pair(map: &mut BTreeMap<String, String>, key: &str, value: &str) {
    map.retain(|_, existing| existing != value);
    map.insert(key.to_string(), value.to_string());
}

/// Fits an image inside a box without changing its aspect ratio.
pub(crate) fn fit_image(width: u32, height: u32, max_width: f32, max_height: f32) -> (f32, f32) {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let scale = (max_width / width).min(max_height / height).min(1.0);
    ((width * scale).max(1.0), (height * scale).max(1.0))
}

/// A representative point inside a hotspot region, used by the keyboard /
/// button alternative to clicking the image.
pub(crate) fn region_center(shape: &HotspotShape) -> (f64, f64) {
    match shape {
        HotspotShape::Circle { cx, cy, .. } => (*cx, *cy),
        HotspotShape::Rect { x, y, w, h } => (x + w / 2.0, y + h / 2.0),
        HotspotShape::Polygon { points } => {
            let count = points.len().max(1) as f64;
            (
                points.iter().map(|point| point[0]).sum::<f64>() / count,
                points.iter().map(|point| point[1]).sum::<f64>() / count,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn moving_an_item_reorders_ids_not_labels() {
        let mut order = ids(&["a", "b", "c", "d"]);
        assert!(move_order(&mut order, "d", 0));
        assert_eq!(order, ids(&["d", "a", "b", "c"]));
        assert!(move_order(&mut order, "a", 3));
        assert_eq!(order, ids(&["d", "b", "c", "a"]));
        assert!(!move_order(&mut order, "missing", 0));
        assert_eq!(order, ids(&["d", "b", "c", "a"]));
    }

    #[test]
    fn moving_to_a_clamped_target_never_loses_an_item() {
        let mut order = ids(&["a", "b", "c"]);
        assert!(move_order(&mut order, "a", 99));
        assert_eq!(order, ids(&["b", "c", "a"]));
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn assigning_a_used_value_moves_it_instead_of_duplicating() {
        let mut map = BTreeMap::from([
            ("t1".to_string(), "l1".to_string()),
            ("t2".to_string(), "l2".to_string()),
        ]);
        assign_pair(&mut map, "t2", "l1");
        assert_eq!(map.get("t2"), Some(&"l1".to_string()));
        assert!(!map.contains_key("t1"));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn fit_image_preserves_aspect_ratio_and_bounds() {
        let (width, height) = fit_image(640, 400, 320.0, 380.0);
        assert!((width / height - 1.6).abs() < 0.001);
        assert!(width <= 320.0 && height <= 380.0);
        let (width, height) = fit_image(100, 1000, 640.0, 380.0);
        assert!(height <= 380.0);
        assert!((width / height - 0.1).abs() < 0.001);
    }

    #[test]
    fn region_centers_fall_inside_convex_regions() {
        for shape in [
            HotspotShape::Circle {
                cx: 0.2,
                cy: 0.3,
                r: 0.1,
            },
            HotspotShape::Rect {
                x: 0.4,
                y: 0.1,
                w: 0.2,
                h: 0.2,
            },
            HotspotShape::Polygon {
                points: vec![[0.1, 0.1], [0.5, 0.1], [0.3, 0.6]],
            },
        ] {
            let (x, y) = region_center(&shape);
            assert!(shape.contains(x, y), "{shape:?}");
        }
    }
}
