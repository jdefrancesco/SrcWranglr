//! Paints a laid-out [`Scene`] in a Sourcetrail-like style: rounded class
//! boxes, colored symbol pills, and smooth wires between pill edges.
//! Pure drawing — no input handling or state changes.

use egui::epaint::{CubicBezierShape, PathShape};
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

use super::camera::Camera;
use super::scene::{
    Group, Pill, Scene, TextRole, Wire, GROUP_FOOTER_HEIGHT, GROUP_PADDING, PILL_PADDING_X,
};
use crate::model::{EdgeKind, SymbolId, SymbolKind};
use crate::style::{BOOKMARK_COLOR, MARK_OUTLINE, NOTE_COLOR};

/// Alpha for wires not touching the hovered symbol.
const DIMMED_WIRE: f32 = 0.3;

const PILL_FONT_SIZE: f32 = 12.0;
const TITLE_FONT_SIZE: f32 = 14.0;
/// Below this on-screen size, text is skipped (it would be unreadable).
const MIN_READABLE_FONT: f32 = 5.0;
const GROUP_CORNER: f32 = 10.0;
const PILL_CORNER: f32 = 7.0;
const WIRE_WIDTH: f32 = 1.6;
const HOT_WIRE_WIDTH: f32 = 2.8;
const ARROW_LENGTH: f32 = 8.0;
const ARROW_HALF_WIDTH: f32 = 4.0;
/// Minimum horizontal pull of a wire's control points (world units).
const WIRE_MIN_BEND: f32 = 40.0;
/// How far a wire between two pills in the same column loops out, plus how
/// much further per unit of vertical distance (capped), so stacked loops
/// don't overlap.
const WIRE_LOOP_OUT: f32 = 24.0;
const WIRE_LOOP_GROWTH: f32 = 0.5;
const WIRE_LOOP_MAX_EXTRA: f32 = 90.0;

/// Height of the legend strip along the canvas bottom; auto-fit leaves it free.
pub const LEGEND_STRIP_HEIGHT: f32 = 26.0;
const LEGEND_PADDING: f32 = 10.0;
const LEGEND_SWATCH: f32 = 20.0;
const LEGEND_SWATCH_GAP: f32 = 6.0;
const LEGEND_ITEM_GAP: f32 = 16.0;
const LEGEND_FONT_SIZE: f32 = 11.0;
/// Note/bookmark dot on a pill or class box, at zoom 1 (and at least).
const BADGE_RADIUS: f32 = 5.0;
const MIN_BADGE_RADIUS: f32 = 3.5;
const BADGE_OUTLINE_WIDTH: f32 = 1.5;
/// A bookmark that also has a note gets a ring this wide in the note color.
const BADGE_RING_WIDTH: f32 = 2.0;

/// A symbol's notes, drawn as an outlined dot on its pill or box: a
/// bookmark, a note, or a bookmark with a note (ringed in the note color).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    pub is_starred: bool,
    pub has_text: bool,
}

/// Canvas colors; follows egui's light/dark theme.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    background: Color32,
    group_fill: Color32,
    group_border: Color32,
    active_border: Color32,
    title: Color32,
    muted: Color32,
    /// Outline of the active pill and of hovered items.
    emphasis: Color32,
    legend_fill: Color32,
    is_dark: bool,
}

impl Palette {
    pub fn for_visuals(visuals: &egui::Visuals) -> Self {
        if visuals.dark_mode {
            Self {
                background: Color32::from_gray(26),
                group_fill: Color32::from_rgb(0x2F, 0x32, 0x37),
                group_border: Color32::from_rgb(0x4A, 0x4F, 0x56),
                active_border: Color32::from_rgb(0xF5, 0xB7, 0x3B),
                title: Color32::from_gray(225),
                muted: Color32::from_gray(150),
                emphasis: Color32::from_gray(240),
                legend_fill: Color32::from_black_alpha(170),
                is_dark: true,
            }
        } else {
            Self {
                background: Color32::from_rgb(0xF6, 0xF6, 0xF7),
                group_fill: Color32::from_rgb(0xE2, 0xE4, 0xE8),
                group_border: Color32::from_rgb(0xC6, 0xCA, 0xD1),
                active_border: Color32::from_rgb(0xE0, 0x9A, 0x10),
                title: Color32::from_gray(40),
                muted: Color32::from_gray(110),
                emphasis: Color32::from_gray(35),
                legend_fill: Color32::from_white_alpha(235),
                is_dark: false,
            }
        }
    }
}

/// Font for text of a given role at zoom 1 (layout measures with this).
pub fn font_for(role: TextRole) -> FontId {
    match role {
        TextRole::Pill => FontId::monospace(PILL_FONT_SIZE),
        TextRole::Title => FontId::proportional(TITLE_FONT_SIZE),
    }
}

fn scaled(role: TextRole, zoom: f32) -> FontId {
    let base = font_for(role);
    FontId::new(base.size * zoom, base.family)
}

/// Fill and text color of a pill, by symbol kind.
pub fn pill_colors(kind: SymbolKind) -> (Color32, Color32) {
    match kind {
        SymbolKind::Function | SymbolKind::Method => (
            Color32::from_rgb(0xF2, 0xC3, 0x5A),
            Color32::from_rgb(0x2A, 0x21, 0x11),
        ),
        SymbolKind::Field => (
            Color32::from_rgb(0x6F, 0xB3, 0xE6),
            Color32::from_rgb(0x0F, 0x22, 0x33),
        ),
        SymbolKind::Class | SymbolKind::Struct => (
            Color32::from_rgb(0xC2, 0xC7, 0xCE),
            Color32::from_rgb(0x1E, 0x1E, 0x1E),
        ),
        SymbolKind::Namespace => (
            Color32::from_rgb(0xB7, 0xA6, 0xE0),
            Color32::from_rgb(0x1E, 0x1A, 0x2A),
        ),
    }
}

/// Wire color by relationship; slightly deeper on light backgrounds.
pub fn wire_color(kind: EdgeKind, palette: &Palette) -> Color32 {
    match (kind, palette.is_dark) {
        (EdgeKind::Calls, true) => Color32::from_rgb(0xE8, 0xB0, 0x4A),
        (EdgeKind::Calls, false) => Color32::from_rgb(0xD8, 0x98, 0x1C),
        (EdgeKind::Uses, true) => Color32::from_rgb(0x5F, 0xA8, 0xDE),
        (EdgeKind::Uses, false) => Color32::from_rgb(0x3A, 0x8C, 0xD0),
        (EdgeKind::Inherits, true) => Color32::from_rgb(0xA0, 0xA6, 0xAE),
        (EdgeKind::Inherits, false) => Color32::from_rgb(0x80, 0x86, 0x90),
        (EdgeKind::Contains, _) => palette.muted,
    }
}

/// Bezier control points (world space) for a wire from `from` to `to`:
/// out of the facing side of each box, or a loop out to the right when both
/// sit in the same column (e.g. a method using a sibling field).
pub fn wire_points(from: Rect, to: Rect) -> [Pos2; 4] {
    let horizontal = |dx: f32| Vec2::new(dx, 0.0);
    if to.center().x > from.right() {
        let (start, end) = (from.right_center(), to.left_center());
        let bend = ((end.x - start.x) / 2.0).max(WIRE_MIN_BEND);
        [start, start + horizontal(bend), end - horizontal(bend), end]
    } else if to.center().x < from.left() {
        let (start, end) = (from.left_center(), to.right_center());
        let bend = ((start.x - end.x) / 2.0).max(WIRE_MIN_BEND);
        [start, start - horizontal(bend), end + horizontal(bend), end]
    } else {
        let (start, end) = (from.right_center(), to.right_center());
        let extra = ((end.y - start.y).abs() * WIRE_LOOP_GROWTH).min(WIRE_LOOP_MAX_EXTRA);
        let out = start.x.max(end.x) + WIRE_LOOP_OUT + extra;
        [start, Pos2::new(out, start.y), Pos2::new(out, end.y), end]
    }
}

/// Everything needed to draw one frame.
pub struct Frame<'a> {
    pub scene: &'a Scene,
    pub camera: Camera,
    /// Screen position of the canvas' top-left corner.
    pub origin: Pos2,
    pub hovered: Option<SymbolId>,
    pub palette: Palette,
    /// Which symbols have notes or bookmarks.
    pub badge_of: &'a dyn Fn(SymbolId) -> Option<Badge>,
}

impl Frame<'_> {
    fn to_screen(&self, world: Pos2) -> Pos2 {
        self.origin + self.camera.world_to_local(world).to_vec2()
    }

    fn rect_to_screen(&self, world: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(world.min), self.to_screen(world.max))
    }

    fn zoom(&self) -> f32 {
        self.camera.zoom
    }
}

pub fn paint(painter: &Painter, frame: &Frame) {
    painter.rect_filled(painter.clip_rect(), 0.0, frame.palette.background);
    for group in &frame.scene.groups {
        paint_group(painter, frame, group);
    }
    paint_wires(painter, frame);
    for pill in frame.scene.all_pills() {
        paint_pill(painter, frame, pill);
    }
    paint_legend(painter, painter.clip_rect(), &frame.palette);
}

fn paint_group(painter: &Painter, frame: &Frame, group: &Group) {
    let zoom = frame.zoom();
    let palette = &frame.palette;
    let rect = frame.rect_to_screen(group.rect);
    let is_active = group.id.is_some() && group.id == frame.scene.active;
    let is_hovered = group.id.is_some() && group.id == frame.hovered;
    let border = match (is_active, is_hovered) {
        (true, _) => Stroke::new(2.0, palette.active_border),
        (false, true) => Stroke::new(1.5, palette.emphasis),
        _ => Stroke::new(1.0, palette.group_border),
    };
    let corner = GROUP_CORNER * zoom;
    painter.rect(rect, corner, palette.group_fill, border, StrokeKind::Inside);

    let title = frame.rect_to_screen(group.title_rect);
    let font = scaled(TextRole::Title, zoom);
    if font.size >= MIN_READABLE_FONT {
        let pos = Pos2::new(title.left() + GROUP_PADDING * zoom, title.center().y);
        painter.text(pos, Align2::LEFT_CENTER, &group.title, font, palette.title);
    }
    if let Some(badge) = group.id.and_then(|id| (frame.badge_of)(id)) {
        paint_badge(painter, title.right_top(), zoom, badge);
    }
    let footer_font = scaled(TextRole::Pill, zoom * 0.9);
    if group.hidden_members > 0 && footer_font.size >= MIN_READABLE_FONT {
        let pos = Pos2::new(
            rect.left() + GROUP_PADDING * zoom,
            rect.bottom() - GROUP_FOOTER_HEIGHT * zoom * 0.5,
        );
        let text = format!("+{} more", group.hidden_members);
        painter.text(pos, Align2::LEFT_CENTER, text, footer_font, palette.muted);
    }
}

fn paint_pill(painter: &Painter, frame: &Frame, pill: &Pill) {
    let zoom = frame.zoom();
    let rect = frame.rect_to_screen(pill.rect);
    let (fill, text_color) = pill_colors(pill.kind);
    let stroke = if frame.scene.active == Some(pill.id) {
        Stroke::new(2.5, frame.palette.emphasis)
    } else if frame.hovered == Some(pill.id) {
        Stroke::new(1.5, frame.palette.emphasis)
    } else {
        Stroke::NONE
    };
    painter.rect(rect, PILL_CORNER * zoom, fill, stroke, StrokeKind::Outside);
    if let Some(badge) = (frame.badge_of)(pill.id) {
        paint_badge(painter, rect.right_top(), zoom, badge);
    }

    let font = scaled(TextRole::Pill, zoom);
    if font.size >= MIN_READABLE_FONT {
        let pos = Pos2::new(rect.left() + PILL_PADDING_X * zoom, rect.center().y);
        painter.text(pos, Align2::LEFT_CENTER, &pill.label, font, text_color);
    }
}

fn paint_badge(painter: &Painter, at: Pos2, zoom: f32, badge: Badge) {
    let radius = (BADGE_RADIUS * zoom).max(MIN_BADGE_RADIUS);
    let (fill, outline) = badge_colors(badge);
    painter.circle(at, radius, fill, outline);
}

/// Fill and outline of a badge.
fn badge_colors(badge: Badge) -> (Color32, Stroke) {
    let outline = Stroke::new(BADGE_OUTLINE_WIDTH, MARK_OUTLINE);
    match (badge.is_starred, badge.has_text) {
        (true, true) => (BOOKMARK_COLOR, Stroke::new(BADGE_RING_WIDTH, NOTE_COLOR)),
        (true, false) => (BOOKMARK_COLOR, outline),
        (false, _) => (NOTE_COLOR, outline),
    }
}

fn paint_wires(painter: &Painter, frame: &Frame) {
    let scene = frame.scene;
    let touches_hovered = |w: &&Wire| frame.hovered.is_some_and(|h| h == w.from || h == w.to);
    let secondary = scene.secondary_wires.iter().filter(touches_hovered);
    for wire in scene.wires.iter().chain(secondary) {
        let (Some(from), Some(to)) = (scene.anchor(wire.from), scene.anchor(wire.to)) else {
            continue;
        };
        let is_hot = frame
            .hovered
            .is_some_and(|h| h == wire.from || h == wire.to);
        let color = wire_color(wire.kind, &frame.palette);
        let (color, width) = match (frame.hovered.is_some(), is_hot) {
            (true, true) => (color, HOT_WIRE_WIDTH),
            (true, false) => (color.gamma_multiply(DIMMED_WIRE), WIRE_WIDTH),
            _ => (color, WIRE_WIDTH),
        };
        let points = wire_points(from, to).map(|p| frame.to_screen(p));
        let curve = CubicBezierShape::from_points_stroke(
            points,
            false,
            Color32::TRANSPARENT,
            Stroke::new(width, color),
        );
        painter.add(curve);
        paint_arrowhead(painter, points[2], points[3], color);
    }
}

/// Filled arrowhead at `tip`, pointing away from `control`.
fn paint_arrowhead(painter: &Painter, control: Pos2, tip: Pos2, color: Color32) {
    let delta = tip - control;
    if delta.length() < f32::EPSILON {
        return;
    }
    let dir = delta.normalized();
    let back = tip - dir * ARROW_LENGTH;
    let normal = Vec2::new(-dir.y, dir.x) * ARROW_HALF_WIDTH;
    let triangle = vec![tip, back + normal, back - normal];
    painter.add(PathShape::convex_polygon(triangle, color, Stroke::NONE));
}

enum Swatch {
    Pill(Color32),
    Wire(Color32),
}

/// One-row legend strip along the bottom edge of the canvas.
fn paint_legend(painter: &Painter, canvas: Rect, palette: &Palette) {
    let items = [
        (
            Swatch::Pill(pill_colors(SymbolKind::Function).0),
            "function / method",
        ),
        (Swatch::Pill(pill_colors(SymbolKind::Field).0), "field"),
        (
            Swatch::Pill(pill_colors(SymbolKind::Class).0),
            "class / struct",
        ),
        (
            Swatch::Pill(pill_colors(SymbolKind::Namespace).0),
            "namespace",
        ),
        (Swatch::Wire(wire_color(EdgeKind::Calls, palette)), "calls"),
        (Swatch::Wire(wire_color(EdgeKind::Uses, palette)), "uses"),
        (
            Swatch::Wire(wire_color(EdgeKind::Inherits, palette)),
            "inherits",
        ),
    ];
    let strip = Rect::from_min_max(
        Pos2::new(canvas.left(), canvas.bottom() - LEGEND_STRIP_HEIGHT),
        canvas.max,
    );
    painter.rect_filled(strip, 0.0, palette.legend_fill);
    let top_border = Stroke::new(1.0, palette.group_border);
    painter.line_segment([strip.left_top(), strip.right_top()], top_border);

    let font = FontId::proportional(LEGEND_FONT_SIZE);
    let y = strip.center().y;
    let mut x = strip.left() + LEGEND_PADDING;
    for (swatch, text) in items {
        let swatch_rect = Rect::from_min_size(
            Pos2::new(x, y - LEGEND_STRIP_HEIGHT * 0.25),
            Vec2::new(LEGEND_SWATCH, LEGEND_STRIP_HEIGHT * 0.5),
        );
        match swatch {
            Swatch::Pill(color) => {
                painter.rect_filled(swatch_rect, 4.0, color);
            }
            Swatch::Wire(color) => {
                let ends = [swatch_rect.left_center(), swatch_rect.right_center()];
                painter.line_segment(ends, Stroke::new(2.0, color));
            }
        }
        x += LEGEND_SWATCH + LEGEND_SWATCH_GAP;
        let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), palette.title);
        let width = galley.size().x;
        painter.galley(
            Pos2::new(x, y - galley.size().y / 2.0),
            galley,
            palette.title,
        );
        x += width + LEGEND_ITEM_GAP;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(60.0, 20.0))
    }

    #[test]
    fn wire_to_the_right_leaves_right_edge_and_enters_left_edge() {
        let (from, to) = (rect(0.0, 0.0), rect(300.0, 50.0));
        let p = wire_points(from, to);
        assert_eq!(p[0], from.right_center());
        assert_eq!(p[3], to.left_center());
        assert!(p[1].x > p[0].x && p[2].x < p[3].x);
    }

    #[test]
    fn wire_to_the_left_leaves_left_edge_and_enters_right_edge() {
        let (from, to) = (rect(300.0, 0.0), rect(0.0, 0.0));
        let p = wire_points(from, to);
        assert_eq!(p[0], from.left_center());
        assert_eq!(p[3], to.right_center());
    }

    #[test]
    fn wire_within_a_column_loops_out_to_the_right() {
        let (from, to) = (rect(0.0, 0.0), rect(5.0, 60.0));
        let p = wire_points(from, to);
        assert_eq!(p[0], from.right_center());
        assert_eq!(p[3], to.right_center());
        assert!(p[1].x > from.right() && p[2].x > to.right());
    }

    #[test]
    fn longer_loops_bulge_further_so_they_do_not_overlap() {
        let from = rect(0.0, 0.0);
        let near = wire_points(from, rect(0.0, 30.0));
        let far = wire_points(from, rect(0.0, 60.0));
        assert!(far[1].x > near[1].x);
    }

    #[test]
    fn short_wires_still_bend_smoothly() {
        let (from, to) = (rect(0.0, 0.0), rect(70.0, 100.0));
        let p = wire_points(from, to);
        assert!((p[1].x - p[0].x) >= WIRE_MIN_BEND);
    }

    #[test]
    fn palette_follows_theme() {
        assert!(Palette::for_visuals(&egui::Visuals::dark()).is_dark);
        assert!(!Palette::for_visuals(&egui::Visuals::light()).is_dark);
    }

    /// Every circle `paint` draws for `scene`, with `badge_of`.
    fn painted_circles(
        scene: &Scene,
        badge_of: &dyn Fn(SymbolId) -> Option<Badge>,
    ) -> Vec<egui::epaint::CircleShape> {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let frame = Frame {
                scene,
                camera: Camera::default(),
                origin: Pos2::ZERO,
                hovered: None,
                palette: Palette::for_visuals(ui.visuals()),
                badge_of,
            };
            paint(ui.painter(), &frame);
        });
        let mut output = output;
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Circle(circle) => Some(circle),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn badges_are_painted_on_marked_pills_only() {
        let pill = |id, y| crate::graph::scene::Pill {
            id,
            label: format!("f{id}"),
            kind: SymbolKind::Method,
            rect: Rect::from_min_size(Pos2::new(10.0, y), Vec2::new(80.0, 22.0)),
        };
        let scene = Scene {
            pills: vec![pill(1, 10.0), pill(2, 40.0), pill(3, 70.0)],
            ..Scene::default()
        };
        let badge_of = |id| match id {
            1 => Some(Badge {
                is_starred: false,
                has_text: true,
            }),
            3 => Some(Badge {
                is_starred: true,
                has_text: false,
            }),
            _ => None,
        };
        let fills: Vec<Color32> = painted_circles(&scene, &badge_of)
            .iter()
            .map(|c| c.fill)
            .collect();
        assert_eq!(fills, [NOTE_COLOR, BOOKMARK_COLOR]);
    }

    #[test]
    fn badge_colors_show_bookmark_note_or_both() {
        let badge = |is_starred, has_text| Badge {
            is_starred,
            has_text,
        };
        let fill_and_stroke = |b| {
            let (fill, stroke) = badge_colors(b);
            (fill, stroke.color)
        };
        assert_eq!(
            fill_and_stroke(badge(true, false)),
            (BOOKMARK_COLOR, MARK_OUTLINE)
        );
        assert_eq!(
            fill_and_stroke(badge(false, true)),
            (NOTE_COLOR, MARK_OUTLINE)
        );
        assert_eq!(
            fill_and_stroke(badge(true, true)),
            (BOOKMARK_COLOR, NOTE_COLOR)
        );
    }
}
