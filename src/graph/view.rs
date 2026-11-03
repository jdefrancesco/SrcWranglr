//! The interactive graph widget, Sourcetrail style: the canvas always shows
//! the selected symbol's *trail* (who uses it on the left, what it uses on
//! the right), or a per-file overview when nothing is selected.
//!
//! Controls:
//! - click any symbol (pill or class title) to make it the active symbol
//! - "Back" returns to the previous symbol; "Overview" clears the selection
//! - scroll or pinch: zoom around the pointer; drag: pan
//! - double-click the background, or "Fit": frame everything

use egui::{Pos2, Response, Sense, Vec2};

use super::camera::Camera;
use super::overview::build_overview;
use super::render::{self, font_for, Frame, Palette, LEGEND_STRIP_HEIGHT};
use super::scene::{Scene, TextRole};
use super::trail::build_trail;
use crate::model::{CodeGraph, SymbolId};

/// Extra world-space margin around the scene when fitting it to the view.
const FIT_PADDING: f32 = 40.0;
/// Zoom change per scrolled point: `factor = exp(scroll * ZOOM_PER_SCROLL_POINT)`.
const ZOOM_PER_SCROLL_POINT: f32 = 0.0015;
const BUTTON_ZOOM_STEP: f32 = 1.25;
/// Back-navigation depth kept.
const MAX_HISTORY: usize = 100;

enum ToolbarAction {
    Back,
    Overview,
    ZoomIn,
    ZoomOut,
    Fit,
}

pub struct GraphView {
    /// Laid-out scene and the selection it was built for.
    scene: Option<(Option<SymbolId>, Scene)>,
    camera: Camera,
    /// Keep the scene framed (e.g. across window resizes) until the user
    /// pans or zooms manually.
    is_auto_fit: bool,
    selected: Option<SymbolId>,
    history: Vec<SymbolId>,
    hovered: Option<SymbolId>,
    /// Connection count for the hovered symbol, so hovering doesn't scan
    /// every edge on every frame.
    hover_connections: Option<(SymbolId, usize)>,
    /// Explanation shown in the toolbar (computed with the last scene).
    notice: Option<String>,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            scene: None,
            camera: Camera::default(),
            is_auto_fit: true,
            selected: None,
            history: Vec::new(),
            hovered: None,
            hover_connections: None,
            notice: None,
        }
    }
}

impl GraphView {
    pub fn selected(&self) -> Option<SymbolId> {
        self.selected
    }

    /// Make `id` the active symbol, remembering the previous one for "Back".
    pub fn select(&mut self, id: SymbolId) {
        if self.selected == Some(id) {
            return;
        }
        if let Some(previous) = self.selected {
            self.history.push(previous);
            if self.history.len() > MAX_HISTORY {
                self.history.remove(0);
            }
        }
        self.selected = Some(id);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, graph: &CodeGraph) -> Response {
        let action = self.show_toolbar(ui);
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let pointer_local = response.hover_pos().map(|p| (p - rect.min).to_pos2());

        let scene = self.take_scene(ui, graph);
        self.handle_pointer(&response, &scene, pointer_local);
        self.apply_toolbar_action(action, rect.size());
        let fit_area = rect.size() - Vec2::new(0.0, LEGEND_STRIP_HEIGHT);
        self.update_camera(&scene, fit_area);
        self.handle_scroll_zoom(ui, &response, pointer_local);

        let painter = ui.painter_at(rect);
        let frame = Frame {
            scene: &scene,
            camera: self.camera,
            origin: rect.min,
            hovered: self.hovered,
            palette: Palette::for_visuals(ui.visuals()),
        };
        render::paint(&painter, &frame);
        self.notice = notice(&scene, graph);
        self.scene = Some((scene.active, scene));

        self.with_hover_tooltip(response, graph)
    }

    /// The scene for the current selection, rebuilt (and re-fitted) when the
    /// selection changed since it was laid out.
    fn take_scene(&mut self, ui: &egui::Ui, graph: &CodeGraph) -> Scene {
        if let Some((built_for, scene)) = self.scene.take() {
            if built_for == self.selected {
                return scene;
            }
        }
        let painter = ui.painter();
        let measure = |text: &str, role: TextRole| {
            let galley =
                painter.layout_no_wrap(text.to_owned(), font_for(role), egui::Color32::WHITE);
            galley.size().x
        };
        self.is_auto_fit = true;
        self.hover_connections = None;
        match self.selected {
            Some(id) => build_trail(graph, id, &measure),
            None => build_overview(graph, &measure),
        }
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui) -> Option<ToolbarAction> {
        ui.horizontal(|ui| {
            let mut action = None;
            let back = egui::Button::new("◀ Back");
            if ui.add_enabled(!self.history.is_empty(), back).clicked() {
                action = Some(ToolbarAction::Back);
            }
            let overview = egui::Button::new("Overview");
            if ui.add_enabled(self.selected.is_some(), overview).clicked() {
                action = Some(ToolbarAction::Overview);
            }
            ui.separator();
            if ui.button("−").on_hover_text("Zoom out").clicked() {
                action = Some(ToolbarAction::ZoomOut);
            }
            ui.label(format!("{:>3.0}%", self.camera.zoom * 100.0));
            if ui.button("+").on_hover_text("Zoom in").clicked() {
                action = Some(ToolbarAction::ZoomIn);
            }
            if ui.button("Fit").on_hover_text("Frame everything").clicked() {
                action = Some(ToolbarAction::Fit);
            }
            ui.separator();
            ui.weak("click: explore · scroll: zoom · drag: pan · double-click: fit");
            if let Some(notice) = &self.notice {
                ui.separator();
                ui.label(egui::RichText::new(notice).italics());
            }
            action
        })
        .inner
    }

    fn apply_toolbar_action(&mut self, action: Option<ToolbarAction>, canvas_size: Vec2) {
        let center = (canvas_size * 0.5).to_pos2();
        let zoom_factor = match action {
            Some(ToolbarAction::Back) => {
                self.selected = self.history.pop();
                return;
            }
            Some(ToolbarAction::Overview) => {
                if let Some(previous) = self.selected.take() {
                    self.history.push(previous);
                }
                return;
            }
            Some(ToolbarAction::Fit) => {
                self.is_auto_fit = true;
                return;
            }
            Some(ToolbarAction::ZoomIn) => BUTTON_ZOOM_STEP,
            Some(ToolbarAction::ZoomOut) => 1.0 / BUTTON_ZOOM_STEP,
            None => return,
        };
        self.camera = self.camera.zoomed_at(zoom_factor, center);
        self.is_auto_fit = false;
    }

    /// Hover, background panning, and click-to-explore.
    fn handle_pointer(&mut self, response: &Response, scene: &Scene, pointer_local: Option<Pos2>) {
        self.hovered = pointer_local
            .map(|p| self.camera.local_to_world(p))
            .and_then(|world| scene.hit(world));

        if response.dragged() {
            self.camera = self.camera.panned(response.drag_delta());
            self.is_auto_fit = false;
        }
        if response.double_clicked() && self.hovered.is_none() {
            self.is_auto_fit = true;
        } else if response.clicked() {
            if let Some(id) = self.hovered {
                self.select(id);
            }
        }
    }

    fn update_camera(&mut self, scene: &Scene, canvas_size: Vec2) {
        if canvas_size.x <= 0.0 || canvas_size.y <= 0.0 || !self.is_auto_fit {
            return;
        }
        if let Some(bounds) = scene.bounds() {
            self.camera = Camera::fit(bounds.expand(FIT_PADDING), canvas_size);
        }
    }

    fn handle_scroll_zoom(
        &mut self,
        ui: &egui::Ui,
        response: &Response,
        pointer_local: Option<Pos2>,
    ) {
        let Some(anchor) = pointer_local.filter(|_| response.hovered()) else {
            return;
        };
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = pinch * (scroll * ZOOM_PER_SCROLL_POINT).exp();
        if (factor - 1.0).abs() > f32::EPSILON {
            self.camera = self.camera.zoomed_at(factor, anchor);
            self.is_auto_fit = false;
        }
    }

    fn with_hover_tooltip(&mut self, response: Response, graph: &CodeGraph) -> Response {
        let Some(symbol) = self.hovered.and_then(|id| graph.symbols.get(&id)) else {
            return response;
        };
        let connections = match self.hover_connections {
            Some((id, count)) if id == symbol.id => count,
            _ => {
                let count = graph.edges_for(symbol.id).count();
                self.hover_connections = Some((symbol.id, count));
                count
            }
        };
        response.on_hover_ui_at_pointer(|ui| {
            ui.strong(&symbol.qualified_name);
            ui.label(format!(
                "{} · {}:{}",
                symbol.kind.label(),
                symbol.file,
                symbol.line
            ));
            ui.weak(format!("{connections} connection(s) · click to explore"));
        })
    }
}

/// Text overlaid on the canvas when the view needs explaining.
fn notice(scene: &Scene, graph: &CodeGraph) -> Option<String> {
    if graph.symbols.is_empty() {
        return Some("No symbols — open a project with File → Open Project…".into());
    }
    match (scene.active, scene.hidden_neighbors) {
        (Some(_), 0) => None,
        (Some(_), hidden) => Some(format!("{hidden} more related symbols not shown")),
        (None, 0) => Some("Click a symbol to explore its relationships".into()),
        (None, hidden) => Some(format!(
            "Click a symbol to explore · {hidden} symbols in more files not shown"
        )),
    }
}
