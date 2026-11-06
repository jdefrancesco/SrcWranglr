//! The start page, shown until a project is open: open a folder, reopen a
//! recent one, or try the built-in demo.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Recent projects remembered.
pub const MAX_RECENT: usize = 8;

/// Project folders opened before, most recent first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProjects {
    paths: Vec<PathBuf>,
}

impl RecentProjects {
    /// Where the list is kept in eframe's storage.
    pub const STORAGE_KEY: &'static str = "recent_projects";

    /// The list saved in `storage`: absolute paths only, each once, at
    /// most `MAX_RECENT`.
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let saved: Self = storage
            .and_then(|s| eframe::get_value(s, Self::STORAGE_KEY))
            .unwrap_or_default();
        saved
            .paths
            .iter()
            .rev()
            .filter(|p| p.is_absolute())
            .fold(Self::default(), |list, p| list.with_opened(p))
    }

    /// Keeps the list in `storage` for the next launch.
    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, Self::STORAGE_KEY, self);
    }

    /// The list with `root` first.
    pub fn with_opened(&self, root: &Path) -> Self {
        let paths = std::iter::once(root.to_path_buf())
            .chain(self.paths.iter().filter(|p| *p != root).cloned())
            .take(MAX_RECENT)
            .collect();
        Self { paths }
    }

    /// The list without `root`.
    pub fn without(&self, root: &Path) -> Self {
        Self {
            paths: self.paths.iter().filter(|p| *p != root).cloned().collect(),
        }
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

/// What was clicked on the start page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WelcomeAction {
    OpenProject,
    OpenRecent(PathBuf),
    Forget(PathBuf),
    OpenDemo,
}

const PAGE_WIDTH: f32 = 520.0;
/// Space above the page, as a share of the room.
const TOP_SHARE: f32 = 0.18;
const OPEN_BUTTON_SIZE: [f32; 2] = [240.0, 36.0];

/// Draws the start page in `ui`. `indexing` is the folder being indexed,
/// if any; `open_keys` the Open Project shortcut, as shown.
pub fn show(
    ui: &mut egui::Ui,
    recent: &RecentProjects,
    indexing: Option<&Path>,
    open_keys: &str,
) -> Option<WelcomeAction> {
    ui.add_space(ui.available_height() * TOP_SHARE);
    ui.vertical_centered(|ui| {
        ui.set_max_width(PAGE_WIDTH);
        ui.heading(egui::RichText::new(super::APP_NAME).size(32.0).strong());
        ui.weak(format!("Version {}", crate::version::version_line()));
        ui.weak("Explore and audit C/C++ source code: call graphs, risky calls, triage and fuzz targets.");
        ui.add_space(24.0);
        if let Some(root) = indexing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Indexing {}…", root.display()));
            });
            ui.add_space(12.0);
        }
        let open = egui::Button::new(egui::RichText::new("Open Project Folder…").size(16.0))
            .min_size(OPEN_BUTTON_SIZE.into());
        let mut action = ui.add(open).clicked().then_some(WelcomeAction::OpenProject);
        ui.weak(format!("or {open_keys}, from anywhere"));
        ui.add_space(24.0);
        action = recent_list(ui, recent).or(action);
        ui.add_space(24.0);
        if ui.link("Try the built-in demo").clicked() {
            action = Some(WelcomeAction::OpenDemo);
        }
        action
    })
    .inner
}

/// The recent projects, each a click away (or, if gone, forgettable).
fn recent_list(ui: &mut egui::Ui, recent: &RecentProjects) -> Option<WelcomeAction> {
    ui.strong("Recent projects");
    ui.add_space(4.0);
    if recent.paths().is_empty() {
        ui.weak("Projects you open will be listed here.");
        return None;
    }
    let mut action = None;
    for path in recent.paths() {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let is_there = path.is_dir();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    is_there,
                    egui::Link::new(egui::RichText::new(&name).strong()),
                )
                .on_hover_text(path.display().to_string())
                .clicked()
            {
                action = Some(WelcomeAction::OpenRecent(path.clone()));
            }
            ui.weak(path.display().to_string());
            if !is_there {
                ui.weak("(not found)");
                if ui.small_button("Forget").clicked() {
                    action = Some(WelcomeAction::Forget(path.clone()));
                }
            }
        });
    }
    action
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use eframe::Storage;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[derive(Default)]
    struct MemoryStorage(HashMap<String, String>);

    impl Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }

        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }

        fn flush(&mut self) {}
    }

    fn recent(paths: &[&str]) -> RecentProjects {
        paths
            .iter()
            .rev()
            .fold(RecentProjects::default(), |list, p| {
                list.with_opened(Path::new(p))
            })
    }

    #[test]
    fn opening_a_project_puts_it_first_once() {
        let list = recent(&["/a", "/b"]).with_opened(Path::new("/b"));
        assert_eq!(list.paths(), [PathBuf::from("/b"), PathBuf::from("/a")]);
    }

    #[test]
    fn only_the_most_recent_are_kept() {
        let names: Vec<String> = (0..MAX_RECENT + 3).map(|i| format!("/p{i}")).collect();
        let list = names.iter().fold(RecentProjects::default(), |list, p| {
            list.with_opened(Path::new(p))
        });
        assert_eq!(list.paths().len(), MAX_RECENT);
        assert_eq!(
            list.paths()[0],
            PathBuf::from(format!("/p{}", MAX_RECENT + 2))
        );
    }

    #[test]
    fn a_project_can_be_forgotten() {
        let list = recent(&["/a", "/b"]).without(Path::new("/a"));
        assert_eq!(list.paths(), [PathBuf::from("/b")]);
    }

    #[test]
    fn the_list_is_kept_between_launches() {
        let mut storage = MemoryStorage::default();
        let list = recent(&["/a", "/b"]);
        list.save(&mut storage);
        assert_eq!(RecentProjects::load(Some(&storage)), list);
    }

    #[test]
    fn a_saved_list_is_cleaned_up_on_load() {
        let mut storage = MemoryStorage::default();
        let paths: Vec<String> = ["\"/a\"", "\"\"", "\"relative\"", "\"/a\""]
            .iter()
            .map(|p| (*p).to_string())
            .chain((0..MAX_RECENT + 2).map(|i| format!("\"/p{i}\"")))
            .collect();
        storage.set_string(
            RecentProjects::STORAGE_KEY,
            format!("(paths: [{}])", paths.join(", ")),
        );
        let list = RecentProjects::load(Some(&storage));
        assert_eq!(list.paths()[0], PathBuf::from("/a"));
        assert_eq!(list.paths().len(), MAX_RECENT);
        assert!(list.paths().iter().all(|p| p.is_absolute()));
        assert_eq!(
            list.paths()
                .iter()
                .filter(|p| **p == Path::new("/a"))
                .count(),
            1
        );
    }

    #[test]
    fn unreadable_storage_means_no_recent_projects() {
        let mut storage = MemoryStorage::default();
        storage.set_string(RecentProjects::STORAGE_KEY, "garbage".into());
        assert_eq!(
            RecentProjects::load(Some(&storage)),
            RecentProjects::default()
        );
    }

    struct State {
        recent: RecentProjects,
        indexing: Option<PathBuf>,
        action: Option<WelcomeAction>,
    }

    fn page(
        recent: RecentProjects,
        indexing: Option<PathBuf>,
    ) -> egui_kittest::Harness<'static, State> {
        let state = State {
            recent,
            indexing,
            action: None,
        };
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1000.0, 700.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    let recent = state.recent.clone();
                    if let Some(action) = show(ui, &recent, state.indexing.as_deref(), "Cmd+O") {
                        state.action = Some(action);
                    }
                },
                state,
            );
        // Fixed steps: a spinner never lets the page settle.
        harness.run_steps(4);
        harness
    }

    fn click(harness: &mut egui_kittest::Harness<'_, State>, label: &str) -> Option<WelcomeAction> {
        harness.get_by_label(label).click();
        harness.run();
        harness.state().action.clone()
    }

    #[test]
    fn the_page_offers_to_open_a_project_or_the_demo() {
        let mut harness = page(RecentProjects::default(), None);
        assert!(harness.query_by_label_contains("Cmd+O").is_some());
        assert!(harness
            .query_by_label_contains("Projects you open")
            .is_some());
        assert_eq!(
            click(&mut harness, "Open Project Folder…"),
            Some(WelcomeAction::OpenProject)
        );
        assert_eq!(
            click(&mut harness, "Try the built-in demo"),
            Some(WelcomeAction::OpenDemo)
        );
    }

    #[test]
    fn the_page_shows_which_version_this_is() {
        let harness = page(RecentProjects::default(), None);
        let version = crate::version::version_line();
        assert!(
            harness.query_by_label_contains(&version).is_some(),
            "{version}"
        );
    }

    #[test]
    fn a_recent_project_reopens_with_a_click() {
        let dir = tempfile::tempdir().expect("tempdir");
        let name = dir
            .path()
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let mut harness = page(RecentProjects::default().with_opened(dir.path()), None);
        assert_eq!(
            click(&mut harness, &name),
            Some(WelcomeAction::OpenRecent(dir.path().to_path_buf()))
        );
    }

    #[test]
    fn a_missing_recent_project_can_only_be_forgotten() {
        let missing = PathBuf::from("/no/such/srcwranglr-project");
        let mut harness = page(RecentProjects::default().with_opened(&missing), None);
        assert!(harness.query_by_label_contains("not found").is_some());
        assert_eq!(
            click(&mut harness, "Forget"),
            Some(WelcomeAction::Forget(missing))
        );
    }

    #[test]
    fn indexing_is_shown_while_it_runs() {
        let harness = page(
            RecentProjects::default(),
            Some(PathBuf::from("/code/leveldb")),
        );
        assert!(harness
            .query_by_label_contains("Indexing /code/leveldb")
            .is_some());
    }
}
