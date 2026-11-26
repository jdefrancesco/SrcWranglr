//! The History pane's CVEs tab: each CVE id the fix commits name, with
//! those commits and the files they changed. The NVD link opens the CVE's
//! page in your browser when clicked; nothing is looked up otherwise.

use egui::RichText;

use super::history::HistoryView;
use super::history_panel::HistoryPanelAction;
use super::visible::visible;
use crate::history::cves::nvd_url;
use crate::history::date;
use crate::model::CodeGraph;

/// Files listed under a commit, commits under a CVE, and CVEs, at most.
const MAX_FILES_SHOWN: usize = 50;
const MAX_COMMITS_SHOWN: usize = 10;
const MAX_CVES_SHOWN: usize = 300;

/// What was picked in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CveAction {
    /// Show this fix commit (by index) in the Fix commits tab.
    ShowCommit(usize),
    GoTo(HistoryPanelAction),
}

pub fn cve_list(ui: &mut egui::Ui, view: &HistoryView, graph: &CodeGraph) -> Option<CveAction> {
    if view.cves.is_empty() {
        ui.weak(format!(
            "No CVE ids in the messages of the {} commits read. Fixes are often committed \
             before their CVE is assigned (the Linux kernel's mostly are), so their messages \
             don't name it: look in Fix commits for security words.",
            view.history.commits_read
        ));
        return None;
    }
    ui.label(format!(
        "{} CVEs named in commit messages, most recently fixed first.",
        view.cves.len()
    ));
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("history_cves")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for cve in view.cves.iter().take(MAX_CVES_SHOWN) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(cve.id.as_str()).monospace().strong());
                    if let Some(url) = nvd_url(&cve.id) {
                        ui.hyperlink_to("NVD ↗", url)
                            .on_hover_text("Open this CVE's page on nvd.nist.gov in your browser");
                    }
                });
                for &index in cve.commits.iter().take(MAX_COMMITS_SHOWN) {
                    picked = commit(ui, view, graph, &cve.id, index).or(picked.take());
                }
                if cve.commits.len() > MAX_COMMITS_SHOWN {
                    let more = cve.commits.len() - MAX_COMMITS_SHOWN;
                    ui.indent(("cve_more", &cve.id), |ui| {
                        ui.weak(format!("and {more} more commits"))
                    });
                }
                ui.add_space(6.0);
            }
            if view.cves.len() > MAX_CVES_SHOWN {
                ui.weak(format!(
                    "and {} more CVEs (filter Fix commits by \"cve-\" to see them)",
                    view.cves.len() - MAX_CVES_SHOWN
                ));
            }
        });
    picked
}

/// The salt for a commit's "files changed" header, made unique per CVE: one
/// commit can fix several CVEs and so appear more than once in the list, so
/// the commit index alone would clash (egui then paints a loud id-clash
/// warning over the rows). egui hashes the salt into the header's id.
fn files_header_salt(cve: &str, index: usize) -> (&'static str, &str, usize) {
    ("cve_files_list", cve, index)
}

/// One commit naming a CVE: its date, id and summary (click to see it in
/// Fix commits), and the files it changed. `cve` makes the widget ids
/// unique, since one commit can fix several CVEs and so appear more than
/// once in the list (a shared index alone would clash).
fn commit(
    ui: &mut egui::Ui,
    view: &HistoryView,
    graph: &CodeGraph,
    cve: &str,
    index: usize,
) -> Option<CveAction> {
    let commit = view.history.fix_commits.get(index)?;
    let mut picked = None;
    let line = format!(
        "{}  {}  {}",
        date(commit.time),
        commit.short,
        commit.summary
    );
    let row = ui
        .indent(("cve_commit", cve, index), |ui| {
            ui.selectable_label(false, visible(&line))
        })
        .inner;
    if row
        .on_hover_text("Show this commit and the lines it last changed in Fix commits")
        .clicked()
    {
        picked = Some(CveAction::ShowCommit(index));
    }
    if commit.files.is_empty() {
        return picked;
    }
    let title = match commit.files.len() {
        1 => "1 file changed".to_string(),
        n => format!("{n} files changed"),
    };
    ui.indent(("cve_files", cve, index), |ui| {
        egui::CollapsingHeader::new(title)
            .id_salt(files_header_salt(cve, index))
            .show(ui, |ui| {
                for file in commit.files.iter().take(MAX_FILES_SHOWN) {
                    let is_in_project = graph.file_contents.contains_key(file);
                    let label = ui.add_enabled(
                        is_in_project,
                        egui::Button::selectable(false, visible(file)),
                    );
                    if label.clicked() {
                        picked = Some(CveAction::GoTo(HistoryPanelAction::GoTo {
                            symbol: None,
                            file: file.clone(),
                            line: 1,
                        }));
                    }
                }
                if commit.files.len() > MAX_FILES_SHOWN {
                    ui.weak(format!("and {} more", commit.files.len() - MAX_FILES_SHOWN));
                }
            });
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_commit_under_two_cves_gets_distinct_header_salts() {
        // The collision that painted an id-clash warning over the list: a
        // commit fixing two CVEs keyed its "files changed" header by index
        // alone, so both appearances hashed to one id.
        let a = files_header_salt("cve-2024-1111", 7);
        let b = files_header_salt("cve-2024-2222", 7);
        assert_ne!(a, b, "shared commit must not reuse one header salt");
        // The same commit under the same CVE is still stable.
        assert_eq!(a, files_header_salt("cve-2024-1111", 7));
    }
}
