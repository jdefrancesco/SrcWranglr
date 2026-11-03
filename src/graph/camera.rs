//! Pan/zoom camera for the graph canvas.
//!
//! Maps between *world* space (where the layout stores node positions) and
//! *local* space (pixels relative to the canvas' top-left corner):
//!
//! ```text
//! local = offset + world * zoom
//! ```
//!
//! All operations return a new `Camera` rather than mutating in place.

use egui::{Pos2, Rect, Vec2};

pub const MIN_ZOOM: f32 = 0.2;
pub const MAX_ZOOM: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub offset: Vec2,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera {
    pub fn world_to_local(&self, world: Pos2) -> Pos2 {
        (self.offset + world.to_vec2() * self.zoom).to_pos2()
    }

    pub fn local_to_world(&self, local: Pos2) -> Pos2 {
        ((local.to_vec2() - self.offset) / self.zoom).to_pos2()
    }

    pub fn panned(self, delta: Vec2) -> Self {
        Self {
            offset: self.offset + delta,
            ..self
        }
    }

    /// Zoom by `factor`, keeping the world point under `anchor` (local space) fixed.
    pub fn zoomed_at(self, factor: f32, anchor: Pos2) -> Self {
        if !factor.is_finite() || factor <= 0.0 {
            return self;
        }
        let world_under_anchor = self.local_to_world(anchor);
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let offset = anchor.to_vec2() - world_under_anchor.to_vec2() * zoom;
        Self { offset, zoom }
    }

    /// Shift so `world` lands at the center of a viewport of `viewport_size`.
    pub fn centered_on(self, world: Pos2, viewport_size: Vec2) -> Self {
        let offset = viewport_size * 0.5 - world.to_vec2() * self.zoom;
        Self { offset, ..self }
    }

    /// Camera that fits `bounds` (world space, already padded by the caller)
    /// inside `viewport_size`, never zooming in past 1:1.
    pub fn fit(bounds: Rect, viewport_size: Vec2) -> Self {
        let size = bounds.size().max(Vec2::splat(1.0));
        let scale = (viewport_size.x / size.x).min(viewport_size.y / size.y);
        let zoom = if scale.is_finite() {
            scale.clamp(MIN_ZOOM, 1.0)
        } else {
            1.0
        };
        Self {
            offset: Vec2::ZERO,
            zoom,
        }
        .centered_on(bounds.center(), viewport_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-3;

    fn assert_pos_eq(a: Pos2, b: Pos2) {
        assert!((a - b).length() < EPS, "{a:?} != {b:?}");
    }

    #[test]
    fn default_camera_is_identity() {
        let cam = Camera::default();
        let p = Pos2::new(12.0, -7.0);
        assert_pos_eq(cam.world_to_local(p), p);
        assert_pos_eq(cam.local_to_world(p), p);
    }

    #[test]
    fn local_to_world_inverts_world_to_local() {
        let cam = Camera {
            offset: Vec2::new(40.0, -15.0),
            zoom: 2.5,
        };
        let world = Pos2::new(3.0, 9.0);
        assert_pos_eq(cam.local_to_world(cam.world_to_local(world)), world);
    }

    #[test]
    fn panned_moves_world_points_by_delta_in_local_space() {
        let cam = Camera {
            offset: Vec2::ZERO,
            zoom: 2.0,
        };
        let world = Pos2::new(5.0, 5.0);
        let before = cam.world_to_local(world);
        let after = cam.panned(Vec2::new(10.0, -4.0)).world_to_local(world);
        assert_pos_eq(after, before + Vec2::new(10.0, -4.0));
    }

    #[test]
    fn zoomed_at_keeps_anchor_point_fixed() {
        let cam = Camera {
            offset: Vec2::new(20.0, 30.0),
            zoom: 1.0,
        };
        let anchor = Pos2::new(100.0, 80.0);
        let world_under_anchor = cam.local_to_world(anchor);
        let zoomed = cam.zoomed_at(1.5, anchor);
        assert!((zoomed.zoom - 1.5).abs() < EPS);
        assert_pos_eq(zoomed.world_to_local(world_under_anchor), anchor);
    }

    #[test]
    fn zoomed_at_clamps_to_zoom_limits() {
        let anchor = Pos2::new(50.0, 50.0);
        assert!((Camera::default().zoomed_at(100.0, anchor).zoom - MAX_ZOOM).abs() < EPS);
        assert!((Camera::default().zoomed_at(0.001, anchor).zoom - MIN_ZOOM).abs() < EPS);
    }

    #[test]
    fn zoomed_at_ignores_non_finite_or_non_positive_factor() {
        let cam = Camera {
            offset: Vec2::new(3.0, 4.0),
            zoom: 1.2,
        };
        let anchor = Pos2::new(10.0, 10.0);
        assert_eq!(cam.zoomed_at(f32::NAN, anchor), cam);
        assert_eq!(cam.zoomed_at(0.0, anchor), cam);
        assert_eq!(cam.zoomed_at(-2.0, anchor), cam);
    }

    #[test]
    fn centered_on_puts_world_point_at_viewport_center() {
        let cam = Camera {
            offset: Vec2::new(-30.0, 12.0),
            zoom: 1.7,
        };
        let size = Vec2::new(400.0, 300.0);
        let world = Pos2::new(77.0, -21.0);
        let centered = cam.centered_on(world, size);
        assert_eq!(centered.zoom, cam.zoom);
        assert_pos_eq(centered.world_to_local(world), (size * 0.5).to_pos2());
    }

    #[test]
    fn fit_shrinks_large_bounds_into_viewport() {
        let bounds = Rect::from_min_max(Pos2::new(-500.0, -100.0), Pos2::new(500.0, 100.0));
        let size = Vec2::new(500.0, 400.0);
        let cam = Camera::fit(bounds, size);
        assert!((cam.zoom - 0.5).abs() < EPS);
        let viewport = Rect::from_min_size(Pos2::ZERO, size);
        assert!(viewport
            .expand(EPS)
            .contains(cam.world_to_local(bounds.min)));
        assert!(viewport
            .expand(EPS)
            .contains(cam.world_to_local(bounds.max)));
        assert_pos_eq(cam.world_to_local(bounds.center()), viewport.center());
    }

    #[test]
    fn fit_does_not_zoom_in_past_one_for_small_bounds() {
        let bounds = Rect::from_center_size(Pos2::new(10.0, 10.0), Vec2::splat(20.0));
        let size = Vec2::new(800.0, 600.0);
        let cam = Camera::fit(bounds, size);
        assert!((cam.zoom - 1.0).abs() < EPS);
        assert_pos_eq(cam.world_to_local(bounds.center()), Pos2::new(400.0, 300.0));
    }

    #[test]
    fn fit_handles_degenerate_viewport() {
        let bounds = Rect::from_center_size(Pos2::ZERO, Vec2::splat(100.0));
        let cam = Camera::fit(bounds, Vec2::ZERO);
        assert!(cam.zoom.is_finite() && cam.zoom >= MIN_ZOOM);
        assert!(cam.offset.x.is_finite() && cam.offset.y.is_finite());
    }
}
