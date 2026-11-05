//! The symbol list on the left: filter by text and kind, sort by name,
//! kind, file position, or how many other symbols use each one.

use std::cmp::Reverse;

use super::notebook::Notebook;
use super::project::Project;
use crate::model::{Symbol, SymbolId, SymbolKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    Name,
    Kind,
    File,
    MostUsed,
}

impl SortOrder {
    const ALL: [SortOrder; 4] = [
        SortOrder::Name,
        SortOrder::Kind,
        SortOrder::File,
        SortOrder::MostUsed,
    ];

    fn label(self) -> &'static str {
        match self {
            SortOrder::Name => "Name",
            SortOrder::Kind => "Kind",
            SortOrder::File => "File",
            SortOrder::MostUsed => "Most used",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KindFilter {
    #[default]
    All,
    Functions,
    Types,
    Fields,
    Namespaces,
}

impl KindFilter {
    const ALL: [KindFilter; 5] = [
        KindFilter::All,
        KindFilter::Functions,
        KindFilter::Types,
        KindFilter::Fields,
        KindFilter::Namespaces,
    ];

    fn label(self) -> &'static str {
        match self {
            KindFilter::All => "All kinds",
            KindFilter::Functions => "Functions",
            KindFilter::Types => "Classes & structs",
            KindFilter::Fields => "Fields",
            KindFilter::Namespaces => "Namespaces",
        }
    }

    fn accepts(self, kind: SymbolKind) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Functions => matches!(kind, SymbolKind::Function | SymbolKind::Method),
            KindFilter::Types => matches!(kind, SymbolKind::Class | SymbolKind::Struct),
            KindFilter::Fields => kind == SymbolKind::Field,
            KindFilter::Namespaces => kind == SymbolKind::Namespace,
        }
    }
}

/// What the list shows: a case-insensitive substring filter, a kind
/// filter, and an order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    pub text: String,
    pub sort: SortOrder,
    pub kinds: KindFilter,
}

/// The ids `query` lists, in its order. Ties keep name order.
pub fn list_symbols(project: &Project, query: &ListQuery) -> Vec<SymbolId> {
    let needle = query.text.trim().to_lowercase();
    let mut rows: Vec<&Symbol> = project
        .symbols_by_name()
        .filter(|s| query.kinds.accepts(s.kind))
        .filter(|s| contains_ignoring_case(&s.qualified_name, &needle))
        .collect();
    // Stable sorts of name-ordered rows: ties stay in name order.
    match query.sort {
        SortOrder::Name => {}
        SortOrder::Kind => rows.sort_by_key(|s| kind_rank(s.kind)),
        SortOrder::File => rows.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line))),
        SortOrder::MostUsed => rows.sort_by_key(|s| Reverse(project.usage(s.id))),
    }
    rows.into_iter().map(|s| s.id).collect()
}

/// Whether `haystack` contains `lowercase_needle`, ignoring case. Doesn't
/// allocate for ASCII needles (the usual case, run per symbol per keystroke).
fn contains_ignoring_case(haystack: &str, lowercase_needle: &str) -> bool {
    if lowercase_needle.is_empty() {
        return true;
    }
    if !lowercase_needle.is_ascii() {
        return haystack.to_lowercase().contains(lowercase_needle);
    }
    let needle = lowercase_needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

/// Where "Kind" order puts each kind: code first, then types and data.
fn kind_rank(kind: SymbolKind) -> u8 {
    match kind {
        SymbolKind::Function => 0,
        SymbolKind::Method => 1,
        SymbolKind::Class => 2,
        SymbolKind::Struct => 3,
        SymbolKind::Field => 4,
        SymbolKind::Namespace => 5,
    }
}

/// The left panel's state: the query and the rows computed for it.
#[derive(Default)]
pub struct SymbolList {
    query: ListQuery,
    rows: Vec<SymbolId>,
    /// The query `rows` were computed for; `None` means stale.
    rows_for: Option<ListQuery>,
}

impl SymbolList {
    /// Recompute rows on the next frame (a new project was loaded). The
    /// query and order are kept.
    pub fn invalidate(&mut self) {
        self.rows_for = None;
    }

    /// Draws the controls and rows; returns the symbol the user clicked.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        selected: Option<SymbolId>,
        notebook: &Notebook,
    ) -> Option<SymbolId> {
        self.controls(ui);
        if self.refresh(project) {
            // The heading above showed the old count.
            ui.ctx().request_repaint();
        }
        ui.separator();
        let row_height = ui.spacing().interact_size.y;
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, self.rows.len(), |ui, range| {
                // Rows must stay one line high for `show_rows` to place them.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for id in &self.rows[range] {
                    let Some(symbol) = project.graph.symbols.get(id) else {
                        continue;
                    };
                    let label = format!(
                        "{}{}",
                        mark_prefix(notebook, *id),
                        row_label(symbol, self.query.sort, project)
                    );
                    if ui.selectable_label(selected == Some(*id), label).clicked() {
                        clicked = Some(*id);
                    }
                }
            });
        clicked
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let count = self.rows.len();
        ui.heading(format!("Symbols ({count})"));
        let filter = egui::TextEdit::singleline(&mut self.query.text)
            .hint_text("Filter symbols…")
            .desired_width(f32::INFINITY);
        ui.add(filter);
        ui.horizontal(|ui| {
            let sort = ui.label("Sort");
            egui::ComboBox::from_id_salt("symbol_sort")
                .selected_text(self.query.sort.label())
                .show_ui(ui, |ui| {
                    for order in SortOrder::ALL {
                        ui.selectable_value(&mut self.query.sort, order, order.label());
                    }
                })
                .response
                .labelled_by(sort.id);
            let show = ui.label("Show");
            egui::ComboBox::from_id_salt("symbol_kinds")
                .selected_text(self.query.kinds.label())
                .show_ui(ui, |ui| {
                    for kinds in KindFilter::ALL {
                        ui.selectable_value(&mut self.query.kinds, kinds, kinds.label());
                    }
                })
                .response
                .labelled_by(show.id);
        });
    }

    /// Recomputes the rows if the query changed. Returns whether it did.
    fn refresh(&mut self, project: &Project) -> bool {
        if self.rows_for.as_ref() == Some(&self.query) {
            return false;
        }
        self.rows = list_symbols(project, &self.query);
        self.rows_for = Some(self.query.clone());
        true
    }
}

/// `★ ` for a bookmarked symbol, `• ` for one with a note.
fn mark_prefix(notebook: &Notebook, id: SymbolId) -> &'static str {
    match notebook.mark(id) {
        Some(mark) if mark.is_starred => "★ ",
        Some(mark) if mark.has_text => "• ",
        _ => "",
    }
}

/// `name  [kind]`, plus what the list is sorted by when that isn't
/// obvious. The sort key comes first when it's long (a file path), so a
/// narrow panel truncates the name rather than the key.
fn row_label(symbol: &Symbol, sort: SortOrder, project: &Project) -> String {
    let base = format!("{}  [{}]", symbol.qualified_name, symbol.kind.label());
    match sort {
        SortOrder::Name | SortOrder::Kind => base,
        SortOrder::File => format!("{}:{}  {base}", symbol.file, symbol.line),
        SortOrder::MostUsed => format!("{base}  ×{}", project.usage(symbol.id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Symbol;

    fn demo() -> Project {
        Project::demo().expect("demo")
    }

    fn listed<'a>(project: &'a Project, query: &ListQuery) -> Vec<&'a Symbol> {
        list_symbols(project, query)
            .iter()
            .map(|id| &project.graph.symbols[id])
            .collect()
    }

    fn query(text: &str, sort: SortOrder, kinds: KindFilter) -> ListQuery {
        ListQuery {
            text: text.into(),
            sort,
            kinds,
        }
    }

    #[test]
    fn default_query_lists_every_symbol_by_name() {
        let project = demo();
        let rows = listed(&project, &ListQuery::default());
        assert_eq!(rows.len(), project.graph.symbols.len());
        assert!(rows
            .windows(2)
            .all(|w| w[0].qualified_name <= w[1].qualified_name));
    }

    #[test]
    fn text_filter_is_a_trimmed_case_insensitive_substring() {
        let project = demo();
        let rows = listed(
            &project,
            &query("  cIRcle::A ", SortOrder::Name, KindFilter::All),
        );
        let names: Vec<&str> = rows.iter().map(|s| s.qualified_name.as_str()).collect();
        assert_eq!(names, ["Circle::area"]);
        let none = listed(
            &project,
            &query("zzz_nothing", SortOrder::Name, KindFilter::All),
        );
        assert!(none.is_empty());
    }

    #[test]
    fn case_insensitive_match_handles_ascii_and_unicode() {
        assert!(contains_ignoring_case(
            "leveldb::DBImpl::Write",
            "dbimpl::w"
        ));
        assert!(!contains_ignoring_case("leveldb::DBImpl", "dbimpl::w"));
        assert!(contains_ignoring_case("Ünïcode::Größe", "größe"));
        assert!(contains_ignoring_case("anything", ""));
        assert!(!contains_ignoring_case("ab", "abc"));
    }

    #[test]
    fn kind_filter_keeps_only_matching_kinds() {
        let project = demo();
        let functions = listed(&project, &query("", SortOrder::Name, KindFilter::Functions));
        assert!(!functions.is_empty());
        assert!(functions
            .iter()
            .all(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method)));
        let types = listed(&project, &query("", SortOrder::Name, KindFilter::Types));
        let type_names: Vec<&str> = types.iter().map(|s| s.qualified_name.as_str()).collect();
        assert_eq!(type_names, ["Circle", "Shape", "Square"]);
        let fields = listed(&project, &query("", SortOrder::Name, KindFilter::Fields));
        assert!(fields.iter().all(|s| s.kind == SymbolKind::Field));
    }

    #[test]
    fn kind_order_lists_functions_first_then_by_name() {
        let project = demo();
        let rows = listed(&project, &query("", SortOrder::Kind, KindFilter::All));
        let ranks: Vec<u8> = rows.iter().map(|s| kind_rank(s.kind)).collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(rows[0].kind, SymbolKind::Function);
        assert!(rows
            .windows(2)
            .all(|w| w[0].kind != w[1].kind || w[0].qualified_name <= w[1].qualified_name));
    }

    #[test]
    fn file_order_follows_file_then_line() {
        let project = demo();
        let rows = listed(&project, &query("", SortOrder::File, KindFilter::All));
        let positions: Vec<(&str, u32)> = rows.iter().map(|s| (s.file.as_str(), s.line)).collect();
        assert!(positions.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn most_used_lists_the_most_referenced_first() {
        let project = demo();
        let rows = listed(&project, &query("", SortOrder::MostUsed, KindFilter::All));
        let usage: Vec<usize> = rows.iter().map(|s| project.usage(s.id)).collect();
        assert!(usage.windows(2).all(|w| w[0] >= w[1]));
        assert!(usage[0] >= 2, "{usage:?}");
    }

    mod ui {
        use egui_kittest::kittest::Queryable;
        use egui_kittest::Harness;

        use super::*;
        use crate::app::notebook::Notebook;
        use crate::notes::NoteTarget;

        struct State {
            list: SymbolList,
            project: Project,
            notebook: Notebook,
            clicked: Option<SymbolId>,
        }

        fn harness() -> Harness<'static, State> {
            let project = demo();
            let state = State {
                list: SymbolList::default(),
                notebook: Notebook::open(&project),
                project,
                clicked: None,
            };
            // ★ comes from the system fonts in the app; the harness only
            // has egui's bundled ones.
            let mut harness = Harness::builder()
                .allow_missing_glyphs()
                .with_size(egui::vec2(420.0, 600.0))
                .build_ui_state(
                    |ui, state: &mut State| {
                        let clicked = state.list.show(ui, &state.project, None, &state.notebook);
                        if let Some(id) = clicked {
                            state.clicked = Some(id);
                        }
                    },
                    state,
                );
            harness.run();
            harness
        }

        #[test]
        fn bookmarked_and_annotated_symbols_are_marked() {
            let mut harness = harness();
            let state = harness.state_mut();
            let target = |name: &str| NoteTarget::Symbol(name.into());
            state
                .notebook
                .toggle_star(&state.project, target("Circle"))
                .expect("star");
            state
                .notebook
                .set(&state.project, target("Square"), "four sides", false)
                .expect("note");
            harness.run();
            harness.get_by_label("★ Circle  [class]");
            harness.get_by_label("• Square  [class]");
            harness.get_by_label("Shape  [class]");
        }

        #[test]
        fn clicking_a_row_returns_its_symbol() {
            let mut harness = harness();
            harness.get_by_label("total_area  [function]").click();
            harness.run();
            let total_area = harness.state().project.find("total_area");
            assert_eq!(harness.state().clicked, total_area);
        }

        #[test]
        fn typing_in_the_filter_narrows_the_rows() {
            let mut harness = harness();
            harness.get_by_role(egui::Role::TextInput).focus();
            harness.run();
            harness
                .get_by_role(egui::Role::TextInput)
                .type_text("square");
            harness.run();
            harness.get_by_label("Square  [class]");
            assert!(harness.query_by_label("Circle  [class]").is_none());
        }

        #[test]
        fn choosing_most_used_shows_use_counts() {
            let mut harness = harness();
            harness
                .get_by_role_and_label(egui::Role::ComboBox, "Sort")
                .click();
            harness.run();
            harness.get_by_label("Most used").click();
            harness.run();
            assert_eq!(harness.state().list.query.sort, SortOrder::MostUsed);
            harness.get_by_label("Shape  [class]  ×2");
        }

        #[test]
        fn file_order_puts_the_position_first() {
            let mut harness = harness();
            harness
                .get_by_role_and_label(egui::Role::ComboBox, "Sort")
                .click();
            harness.run();
            harness.get_by_label("File").click();
            harness.run();
            harness.get_by_label("main.cpp:8  main  [function]");
        }
    }
}
