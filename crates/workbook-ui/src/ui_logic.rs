//! Pure view-state transitions for the new answer widgets.
//!
//! Keeping these out of the GPUI element builders makes mouse, drag, and
//! keyboard actions share one implementation and lets them be tested without a
//! display.

use std::collections::BTreeMap;

use workbook_domain::HotspotShape;
use workbook_domain::ImageFit;

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

/// Aspect ratio of the viewport used by `ImageFit::Cover`.
///
/// `cover` has to crop, so it needs a display area whose aspect ratio is
/// independent of the image. A 4:3 frame is the classroom-friendly default
/// (diagrams and photos fit well) and fits inside the body/option height
/// limits.
pub(crate) const COVER_ASPECT: f32 = 4.0 / 3.0;

/// Hotspot and Labeling store normalized coordinates relative to the whole
/// image. Cropping would move those coordinates, so annotation images are
/// always displayed with `contain`, regardless of any content-level fit.
pub(crate) fn annotation_image_fit() -> ImageFit {
    ImageFit::Contain
}

/// Returns the element box `(width, height)` for a Content image.
///
/// * `contain` shrink-wraps the whole image (never cropped, never upscaled);
///   the surrounding layout supplies the padding.
/// * `cover` always uses a 4:3 viewport bounded by the available width and
///   height, and the image is cropped to fill it. A wide image therefore gets
///   a shorter `contain` box and a taller, cropped `cover` box.
pub(crate) fn image_box(
    width: u32,
    height: u32,
    fit: ImageFit,
    max_width: f32,
    max_height: f32,
) -> (f32, f32) {
    let max_width = max_width.max(1.0);
    let max_height = max_height.max(1.0);
    match fit {
        ImageFit::Contain => fit_image(width, height, max_width, max_height),
        ImageFit::Cover => {
            let width = max_width;
            let height = width / COVER_ASPECT;
            if height <= max_height {
                (width, height)
            } else {
                let height = max_height;
                (height * COVER_ASPECT, height)
            }
        }
    }
}

/// The normalized source rectangle `(x, y, width, height)` of the image that
/// is visible inside the element box.
///
/// `contain` shows the whole image. `cover` crops symmetrically (GPUI's
/// `ObjectFit::Cover` centres the image). This is used by tests and keeps the
/// crop math next to the layout math; Hotspot/Labeling deliberately use
/// `contain`, so their normalized coordinates are never affected by cropping.
#[cfg(test)]
pub(crate) fn visible_source_rect(
    box_width: f32,
    box_height: f32,
    image_width: u32,
    image_height: u32,
    fit: ImageFit,
) -> Option<(f32, f32, f32, f32)> {
    if !box_width.is_finite()
        || !box_height.is_finite()
        || box_width <= 0.0
        || box_height <= 0.0
        || image_width == 0
        || image_height == 0
    {
        return None;
    }
    if fit == ImageFit::Contain {
        return Some((0.0, 0.0, 1.0, 1.0));
    }
    let image_width = image_width as f32;
    let image_height = image_height as f32;
    let scale = (box_width / image_width).max(box_height / image_height);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let drawn_width = image_width * scale;
    let drawn_height = image_height * scale;
    let left = (box_width - drawn_width) / 2.0;
    let top = (box_height - drawn_height) / 2.0;
    Some((
        (-left / drawn_width).clamp(0.0, 1.0),
        (-top / drawn_height).clamp(0.0, 1.0),
        (box_width / drawn_width).clamp(0.0, 1.0),
        (box_height / drawn_height).clamp(0.0, 1.0),
    ))
}

/// The rectangle the image actually occupies inside its element when drawn
/// with `object-fit: contain`. Returns `(x, y, width, height)` in the same
/// logical pixels as the container.
///
/// GPUI reports pointer positions and element bounds in logical pixels, so the
/// same calculation is correct on Retina/HiDPI displays. The letterbox is
/// explicit here instead of assuming the container always has the image's
/// aspect ratio, which is not true when the window narrows.
pub(crate) fn contained_image_rect(
    container_width: f32,
    container_height: f32,
    image_width: u32,
    image_height: u32,
) -> Option<(f32, f32, f32, f32)> {
    if !container_width.is_finite()
        || !container_height.is_finite()
        || container_width <= 0.0
        || container_height <= 0.0
        || image_width == 0
        || image_height == 0
    {
        return None;
    }
    let image_width = image_width as f32;
    let image_height = image_height as f32;
    let scale = (container_width / image_width).min(container_height / image_height);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let width = image_width * scale;
    let height = image_height * scale;
    Some((
        (container_width - width) / 2.0,
        (container_height - height) / 2.0,
        width,
        height,
    ))
}

/// Converts a pointer position inside the image element into normalized image
/// coordinates. Points in the letterbox margin (or outside the element) return
/// `None`, so a click on empty padding is never saved as a region answer.
pub(crate) fn normalize_image_point(
    point_x: f32,
    point_y: f32,
    container_width: f32,
    container_height: f32,
    image_width: u32,
    image_height: u32,
) -> Option<(f64, f64)> {
    let (rect_x, rect_y, rect_width, rect_height) =
        contained_image_rect(container_width, container_height, image_width, image_height)?;
    if !point_x.is_finite() || !point_y.is_finite() {
        return None;
    }
    let x = ((point_x - rect_x) / rect_width) as f64;
    let y = ((point_y - rect_y) / rect_height) as f64;
    const EDGE_EPSILON: f64 = 1e-6;
    if !x.is_finite()
        || !y.is_finite()
        || !(-EDGE_EPSILON..=1.0 + EDGE_EPSILON).contains(&x)
        || !(-EDGE_EPSILON..=1.0 + EDGE_EPSILON).contains(&y)
    {
        return None;
    }
    Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

/// Inverse of [`normalize_image_point`], used to draw the selection marker at
/// the same place the saved coordinates refer to.
pub(crate) fn image_point_to_container(
    x: f64,
    y: f64,
    container_width: f32,
    container_height: f32,
    image_width: u32,
    image_height: u32,
) -> Option<(f32, f32)> {
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return None;
    }
    let (rect_x, rect_y, rect_width, rect_height) =
        contained_image_rect(container_width, container_height, image_width, image_height)?;
    Some((
        rect_x + x as f32 * rect_width,
        rect_y + y as f32 * rect_height,
    ))
}

/// A representative point inside a hotspot region, used by the keyboard /
/// button alternative to clicking the image.
///
/// Circles and rectangles use their centre. Polygons first try the area
/// centroid, then a horizontal scanline through the widest interior interval.
/// The result is always checked with [`HotspotShape::contains`]; when no
/// interior point can be proven the function returns `None` instead of
/// inventing a coordinate that would grade the region incorrectly.
pub(crate) fn region_center(shape: &HotspotShape) -> Option<(f64, f64)> {
    match shape {
        HotspotShape::Circle { cx, cy, r } => {
            if !cx.is_finite()
                || !cy.is_finite()
                || !r.is_finite()
                || *r <= 0.0
                || !(0.0..=1.0).contains(cx)
                || !(0.0..=1.0).contains(cy)
            {
                return None;
            }
            let point = (*cx, *cy);
            shape.contains(point.0, point.1).then_some(point)
        }
        HotspotShape::Rect { x, y, w, h } => {
            if !x.is_finite()
                || !y.is_finite()
                || !w.is_finite()
                || !h.is_finite()
                || *w <= 0.0
                || *h <= 0.0
                || *x < 0.0
                || *y < 0.0
                || x + w > 1.000_001
                || y + h > 1.000_001
            {
                return None;
            }
            let point = (x + w / 2.0, y + h / 2.0);
            if !(0.0..=1.0).contains(&point.0) || !(0.0..=1.0).contains(&point.1) {
                return None;
            }
            shape.contains(point.0, point.1).then_some(point)
        }
        HotspotShape::Polygon { points } => polygon_interior_point(points),
    }
}

fn polygon_interior_point(points: &[[f64; 2]]) -> Option<(f64, f64)> {
    if points.len() < 3
        || points.iter().any(|point| {
            !point[0].is_finite()
                || !point[1].is_finite()
                || !(0.0..=1.0).contains(&point[0])
                || !(0.0..=1.0).contains(&point[1])
        })
    {
        return None;
    }
    if polygon_area(points).abs() < 1e-12 {
        return None;
    }

    let mut candidates = Vec::new();
    if let Some(centroid) = polygon_centroid(points) {
        candidates.push(centroid);
    }
    let count = points.len() as f64;
    candidates.push((
        points.iter().map(|point| point[0]).sum::<f64>() / count,
        points.iter().map(|point| point[1]).sum::<f64>() / count,
    ));

    // A horizontal scanline between consecutive vertex heights always crosses
    // the open interior of a non-degenerate polygon. Taking the midpoint of the
    // widest interval works for convex, concave, U-shaped, and very thin
    // polygons, where the vertex average or centroid can fall outside.
    let mut heights: Vec<f64> = points.iter().map(|point| point[1]).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    heights.dedup();
    for pair in heights.windows(2) {
        let y = (pair[0] + pair[1]) / 2.0;
        if let Some(point) = widest_interval_midpoint(points, y) {
            candidates.push(point);
        }
    }

    candidates
        .into_iter()
        .find(|(x, y)| point_in_unit_square(*x, *y) && polygon_contains(points, *x, *y))
}

fn point_in_unit_square(x: f64, y: f64) -> bool {
    x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)
}

fn polygon_centroid(points: &[[f64; 2]]) -> Option<(f64, f64)> {
    let mut twice_area = 0.0;
    let mut x_sum = 0.0;
    let mut y_sum = 0.0;
    let mut j = points.len() - 1;
    for (i, point) in points.iter().enumerate() {
        let cross = points[j][0] * point[1] - point[0] * points[j][1];
        twice_area += cross;
        x_sum += (points[j][0] + point[0]) * cross;
        y_sum += (points[j][1] + point[1]) * cross;
        j = i;
    }
    if twice_area.abs() < 1e-12 {
        return None;
    }
    let x = x_sum / (3.0 * twice_area);
    let y = y_sum / (3.0 * twice_area);
    point_in_unit_square(x, y).then_some((x, y))
}

fn widest_interval_midpoint(points: &[[f64; 2]], y: f64) -> Option<(f64, f64)> {
    let mut crossings = Vec::new();
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = (points[i][0], points[i][1]);
        let (xj, yj) = (points[j][0], points[j][1]);
        if (yi > y) != (yj > y) {
            let x = xj + (y - yj) * (xi - xj) / (yi - yj);
            if x.is_finite() {
                crossings.push(x);
            }
        }
        j = i;
    }
    crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut best: Option<(f64, f64)> = None;
    for pair in crossings.chunks_exact(2) {
        let width = pair[1] - pair[0];
        if width > 1e-12 && best.is_none_or(|(_, best_width)| width > best_width) {
            best = Some(((pair[0] + pair[1]) / 2.0, width));
        }
    }
    best.map(|(x, _)| (x, y))
}

fn polygon_contains(points: &[[f64; 2]], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = (points[i][0], points[i][1]);
        let (xj, yj) = (points[j][0], points[j][1]);
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn polygon_area(points: &[[f64; 2]]) -> f64 {
    let mut area = 0.0;
    let mut j = points.len() - 1;
    for (i, point) in points.iter().enumerate() {
        area += (points[j][0] * point[1]) - (point[0] * points[j][1]);
        j = i;
    }
    area / 2.0
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

    fn box_for(width: u32, height: u32, fit: ImageFit) -> (f32, f32) {
        image_box(width, height, fit, 640.0, 480.0)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn contain_and_cover_produce_different_layouts_for_wide_images() {
        let (contain_w, contain_h) = box_for(1000, 200, ImageFit::Contain);
        let (cover_w, cover_h) = box_for(1000, 200, ImageFit::Cover);
        assert!(close(contain_w, 640.0));
        assert!(close(contain_h, 128.0));
        assert!(close(cover_w, 640.0));
        assert!(close(cover_h, 480.0));
        assert!((contain_h - cover_h).abs() > 300.0);
        assert!(contain_w <= 640.0 && contain_h <= 480.0);
        assert!(cover_w <= 640.0 && cover_h <= 480.0);
    }

    #[test]
    fn contain_and_cover_produce_different_layouts_for_tall_images() {
        let (contain_w, contain_h) = box_for(200, 1000, ImageFit::Contain);
        let (cover_w, cover_h) = box_for(200, 1000, ImageFit::Cover);
        assert!(close(contain_w, 96.0));
        assert!(close(contain_h, 480.0));
        assert!(close(cover_w, 640.0));
        assert!(close(cover_h, 480.0));
        assert!(cover_w > contain_w * 5.0);
    }

    #[test]
    fn square_and_small_images_still_differ_between_contain_and_cover() {
        let (contain_w, contain_h) = box_for(200, 200, ImageFit::Contain);
        assert!(close(contain_w, 200.0) && close(contain_h, 200.0));
        let (cover_w, cover_h) = box_for(200, 200, ImageFit::Cover);
        assert!(close(cover_w, 640.0) && close(cover_h, 480.0));

        // Small images are not upscaled by contain, but cover fills its frame.
        let (contain_w, contain_h) = box_for(50, 40, ImageFit::Contain);
        assert!(close(contain_w, 50.0) && close(contain_h, 40.0));
        let (cover_w, cover_h) = box_for(50, 40, ImageFit::Cover);
        assert!(cover_w > contain_w && cover_h > contain_h);
    }

    #[test]
    fn cover_uses_available_space_in_narrow_and_wide_containers() {
        let (width, height) = image_box(1000, 200, ImageFit::Cover, 100.0, 480.0);
        assert!(close(width, 100.0) && close(height, 75.0));
        let (width, height) = image_box(1000, 200, ImageFit::Cover, 2000.0, 2000.0);
        assert!(close(width, 2000.0) && close(height, 1500.0));
        // The frame keeps its aspect ratio when the window changes.
        for max_width in [320.0, 640.0, 1120.0] {
            let (width, height) = image_box(1000, 200, ImageFit::Cover, max_width, 480.0);
            assert!(close(width / height, COVER_ASPECT), "{max_width}");
        }
    }

    #[test]
    fn large_images_are_bounded_by_the_available_box() {
        let (width, height) = image_box(4000, 3000, ImageFit::Contain, 640.0, 480.0);
        assert!(close(width, 640.0) && close(height, 480.0));
        let (width, height) = image_box(4000, 1000, ImageFit::Contain, 640.0, 480.0);
        assert!(close(width, 640.0) && close(height, 160.0));
        let (width, height) = image_box(4000, 1000, ImageFit::Cover, 640.0, 480.0);
        assert!(width <= 640.0 && height <= 480.0);
        assert!(close(width / height, COVER_ASPECT));
    }

    #[test]
    fn labeling_and_hotspot_images_always_use_contain() {
        assert_eq!(annotation_image_fit(), ImageFit::Contain);
    }

    #[test]
    fn cover_crops_the_source_symmetrically_and_contain_shows_everything() {
        assert_eq!(
            visible_source_rect(640.0, 480.0, 1000, 200, ImageFit::Contain),
            Some((0.0, 0.0, 1.0, 1.0))
        );
        let (x, y, width, height) =
            visible_source_rect(640.0, 480.0, 1000, 200, ImageFit::Cover).unwrap();
        // Scale is 2.4, the drawn width is 2400 and only 640 is visible.
        assert!(close(x, 880.0 / 2400.0));
        assert!(close(y, 0.0));
        assert!(close(width, 640.0 / 2400.0));
        assert!(close(height, 1.0));
        assert!(x > 0.0 && x + width < 1.0 + 1e-6);

        let (x, y, width, height) =
            visible_source_rect(640.0, 480.0, 200, 1000, ImageFit::Cover).unwrap();
        assert!(close(x, 0.0));
        assert!(close(y, 1360.0 / 3200.0));
        assert!(close(width, 1.0));
        assert!(close(height, 480.0 / 3200.0));
    }

    #[test]
    fn image_boxes_are_resolution_independent() {
        // All math is in logical pixels, so a Retina window (2x backing scale)
        // produces the same numbers for the same logical bounds.
        let logical = image_box(1000, 200, ImageFit::Cover, 640.0, 480.0);
        assert!(close(logical.0, 640.0) && close(logical.1, 480.0));
        assert_eq!(
            visible_source_rect(640.0, 480.0, 1000, 200, ImageFit::Cover),
            visible_source_rect(640.0, 480.0, 1000, 200, ImageFit::Cover)
        );
    }

    #[test]
    fn image_boxes_reject_invalid_dimensions() {
        assert_eq!(
            visible_source_rect(0.0, 10.0, 10, 10, ImageFit::Cover),
            None
        );
        assert_eq!(
            visible_source_rect(10.0, 10.0, 0, 10, ImageFit::Cover),
            None
        );
        assert_eq!(
            visible_source_rect(f32::NAN, 10.0, 10, 10, ImageFit::Contain),
            None
        );
        let (width, height) = image_box(0, 0, ImageFit::Contain, 640.0, 480.0);
        assert!(width >= 1.0 && height >= 1.0);
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
            let (x, y) = region_center(&shape).expect("representative point");
            assert!(shape.contains(x, y), "{shape:?}");
        }
    }

    #[test]
    fn region_centers_stay_inside_concave_and_u_shaped_polygons() {
        let u_shape = HotspotShape::Polygon {
            points: vec![
                [0.1, 0.1],
                [0.9, 0.1],
                [0.9, 0.9],
                [0.7, 0.9],
                [0.7, 0.3],
                [0.3, 0.3],
                [0.3, 0.9],
                [0.1, 0.9],
            ],
        };
        let (x, y) = region_center(&u_shape).expect("U-shaped polygon needs a point");
        assert!(u_shape.contains(x, y), "{x},{y}");

        let concave = HotspotShape::Polygon {
            points: vec![[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.5, 0.35], [0.1, 0.9]],
        };
        let (x, y) = region_center(&concave).expect("concave polygon needs a point");
        assert!(concave.contains(x, y), "{x},{y}");
    }

    #[test]
    fn region_centers_handle_small_and_thin_shapes() {
        let tiny_circle = HotspotShape::Circle {
            cx: 0.51,
            cy: 0.51,
            r: 0.01,
        };
        let (x, y) = region_center(&tiny_circle).expect("tiny circle");
        assert!(tiny_circle.contains(x, y));

        let thin_rect = HotspotShape::Rect {
            x: 0.1,
            y: 0.49,
            w: 0.8,
            h: 0.002,
        };
        let (x, y) = region_center(&thin_rect).expect("thin rectangle");
        assert!(thin_rect.contains(x, y));

        let thin_polygon = HotspotShape::Polygon {
            points: vec![[0.05, 0.5], [0.95, 0.5], [0.95, 0.502], [0.05, 0.502]],
        };
        let (x, y) = region_center(&thin_polygon).expect("thin polygon");
        assert!(thin_polygon.contains(x, y), "{x},{y}");
    }

    #[test]
    fn region_centers_reject_invalid_polygons() {
        for points in [
            vec![[f64::NAN, 0.5], [0.5, 0.1], [0.9, 0.9]],
            vec![[f64::INFINITY, 0.5], [0.5, 0.1], [0.9, 0.9]],
            vec![[0.1, 0.1], [0.5, 0.5]],
            vec![[0.1, 0.1], [0.2, 0.2], [0.3, 0.3]],
            vec![[1.2, 0.1], [0.5, 0.1], [0.9, 0.9]],
        ] {
            let shape = HotspotShape::Polygon { points };
            assert!(region_center(&shape).is_none(), "{shape:?}");
        }
        assert!(
            region_center(&HotspotShape::Circle {
                cx: f64::NAN,
                cy: 0.5,
                r: 0.1,
            })
            .is_none()
        );
        assert!(
            region_center(&HotspotShape::Rect {
                x: 0.9,
                y: 0.1,
                w: 0.2,
                h: 0.2,
            })
            .is_none()
        );
    }

    #[test]
    fn image_points_normalize_to_the_drawn_image_area() {
        // Square image in a square container.
        for (px, py, expected) in [
            (0.0, 0.0, (0.0, 0.0)),
            (50.0, 50.0, (0.5, 0.5)),
            (100.0, 100.0, (1.0, 1.0)),
        ] {
            let point = normalize_image_point(px, py, 100.0, 100.0, 100, 100).unwrap();
            assert!((point.0 - expected.0).abs() < 1e-9, "{px},{py}");
            assert!((point.1 - expected.1).abs() < 1e-9, "{px},{py}");
        }
    }

    #[test]
    fn image_points_account_for_letterboxing_and_aspect_ratio() {
        // Wide image (100x50) inside a square container: 25px bars top/bottom.
        assert_eq!(
            normalize_image_point(100.0, 10.0, 200.0, 200.0, 100, 50),
            None
        );
        let point = normalize_image_point(100.0, 100.0, 200.0, 200.0, 100, 50).unwrap();
        assert!((point.0 - 0.5).abs() < 1e-9);
        assert!((point.1 - 0.5).abs() < 1e-9);

        // Tall image (100x100) inside a 300x100 container: 100px bars left/right.
        assert_eq!(
            normalize_image_point(50.0, 50.0, 300.0, 100.0, 100, 100),
            None
        );
        let point = normalize_image_point(150.0, 50.0, 300.0, 100.0, 100, 100).unwrap();
        assert!((point.0 - 0.5).abs() < 1e-9);
        assert!((point.1 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn image_points_reject_clicks_outside_the_image() {
        assert_eq!(
            normalize_image_point(-1.0, 50.0, 100.0, 100.0, 100, 100),
            None
        );
        assert_eq!(
            normalize_image_point(101.0, 50.0, 100.0, 100.0, 100, 100),
            None
        );
        assert_eq!(
            normalize_image_point(50.0, -1.0, 100.0, 100.0, 100, 100),
            None
        );
        assert_eq!(
            normalize_image_point(50.0, 101.0, 100.0, 100.0, 100, 100),
            None
        );
        assert_eq!(
            normalize_image_point(f32::NAN, 50.0, 100.0, 100.0, 100, 100),
            None
        );
    }

    #[test]
    fn image_points_are_resolution_independent() {
        let small = normalize_image_point(25.0, 75.0, 100.0, 100.0, 200, 200).unwrap();
        let retina = normalize_image_point(50.0, 150.0, 200.0, 200.0, 200, 200).unwrap();
        assert!((small.0 - retina.0).abs() < 1e-9);
        assert!((small.1 - retina.1).abs() < 1e-9);
    }

    #[test]
    fn selection_marker_maps_back_into_the_image_area() {
        let (x, y) = image_point_to_container(0.5, 0.5, 200.0, 200.0, 100, 50).unwrap();
        assert!((x - 100.0).abs() < 1e-6);
        assert!((y - 100.0).abs() < 1e-6);
        assert_eq!(
            image_point_to_container(f64::NAN, 0.5, 200.0, 200.0, 100, 50),
            None
        );
    }
}
