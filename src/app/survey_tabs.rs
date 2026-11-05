//! The Project Triage tabs drawn from the folder scan: languages and size,
//! repository links, and binaries with their exploit mitigations.

use egui::{Color32, RichText};

use super::survey_view::{percent, row, thousands, ERROR_COLOR};
use crate::survey::{
    format_bytes, github_search_url, BinaryReport, Kind, LanguageStats, Status, Survey,
};

const GOOD_COLOR: Color32 = Color32::from_rgb(0x4C, 0xC3, 0x8A);
const WEAK_COLOR: Color32 = Color32::from_rgb(0xD9, 0xA6, 0x4A);
const SHARE_BAR_WIDTH: f32 = 140.0;
/// Binaries listed expanded when there are at most this many.
const MAX_EXPANDED_BINARIES: usize = 3;

pub fn languages_tab(ui: &mut egui::Ui, survey: &Survey) {
    let code = survey.code_lines();
    ui.label(format!(
        "{} lines of code ({} comment, {} blank). The folder holds {} in {} files.",
        thousands(code.code),
        thousands(code.comments),
        thousands(code.blank),
        format_bytes(survey.total_bytes),
        thousands(survey.file_count as u64)
    ));
    ui.weak("Counts cover files the project tracks (git-ignored files are skipped).");
    ui.add_space(6.0);
    let (source, other): (Vec<&LanguageStats>, Vec<&LanguageStats>) =
        survey.languages.iter().partition(|l| l.kind == Kind::Code);
    if source.is_empty() {
        ui.label("No source files recognized.");
    } else {
        language_grid(ui, "survey_code_languages", &source, code.code);
    }
    if !other.is_empty() {
        ui.add_space(8.0);
        ui.strong("Build files, docs and data");
        language_grid(ui, "survey_other_languages", &other, 0);
    }
}

/// One row per language; `share_of` > 0 adds each one's share of it.
fn language_grid(ui: &mut egui::Ui, id: &str, languages: &[&LanguageStats], share_of: u64) {
    egui::Grid::new(id)
        .num_columns(6)
        .striped(true)
        .spacing([14.0, 4.0])
        .show(ui, |ui| {
            for heading in ["Language", "Files", "Code", "Comments", "Blank"] {
                ui.strong(heading);
            }
            ui.strong(if share_of > 0 { "Share of code" } else { "" });
            ui.end_row();
            for language in languages {
                ui.label(language.name);
                for value in [
                    language.files as u64,
                    language.lines.code,
                    language.lines.comments,
                    language.lines.blank,
                ] {
                    ui.label(thousands(value));
                }
                if share_of > 0 {
                    let share = percent(language.lines.code, share_of);
                    let bar = egui::ProgressBar::new(share as f32 / 100.0)
                        .desired_width(SHARE_BAR_WIDTH)
                        .text(format!("{share}%"));
                    ui.add(bar);
                } else {
                    ui.label("");
                }
                ui.end_row();
            }
        });
}

pub fn links_tab(ui: &mut egui::Ui, survey: &Survey, project_name: &str) {
    ui.weak("Read from the project's git metadata and top-level files. Nothing was fetched.");
    ui.add_space(6.0);
    if survey.links.is_empty() {
        ui.label("No repository links found.");
    }
    for link in &survey.links {
        ui.horizontal_wrapped(|ui| {
            if link.is_web {
                ui.hyperlink_to(&link.url, &link.url);
            } else {
                ui.monospace(&link.url);
            }
            ui.weak(format!("from {}", link.sources.join(", ")));
        });
    }
    ui.add_space(8.0);
    let search = format!("Search GitHub for “{project_name}”");
    ui.hyperlink_to(search, github_search_url(project_name))
        .on_hover_text("Opens github.com in your browser");
}

pub fn binaries_tab(ui: &mut egui::Ui, survey: &Survey) {
    ui.weak("Read from each file's headers and symbols; nothing is run.");
    ui.add_space(6.0);
    if survey.binaries.is_empty() {
        ui.label("No compiled programs or libraries in the folder.");
    }
    let is_expanded = survey.binaries.len() <= MAX_EXPANDED_BINARIES;
    for binary in &survey.binaries {
        let header = format!(
            "{}   {} {} {}, {}",
            binary.path,
            binary.format,
            binary.arch,
            binary.kind,
            format_bytes(binary.size)
        );
        egui::CollapsingHeader::new(header)
            .id_salt(&binary.path)
            .default_open(is_expanded)
            .show(ui, |ui| binary_details(ui, binary));
    }
    if survey.object_files > 0 {
        ui.weak(format!(
            "Also {} object files or static archives (not listed).",
            survey.object_files
        ));
    }
    if !survey.unreadable_binaries.is_empty() {
        let title = format!(
            "Couldn't inspect {} files",
            survey.unreadable_binaries.len()
        );
        egui::CollapsingHeader::new(title).show(ui, |ui| {
            for (path, why) in &survey.unreadable_binaries {
                ui.label(format!("{path}: {why}"));
            }
        });
    }
}

fn binary_details(ui: &mut egui::Ui, binary: &BinaryReport) {
    egui::Grid::new(("binary_checks", &binary.path))
        .num_columns(3)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.strong("Symbols");
            ui.label(status_badge(if binary.is_stripped {
                Status::Weak
            } else {
                Status::Good
            }));
            ui.label(&binary.symbols);
            ui.end_row();
            for check in &binary.checks {
                ui.strong(check.name);
                ui.label(status_badge(check.status));
                ui.label(&check.detail);
                ui.end_row();
            }
        });
    let mut facts = egui::Grid::new(("binary_facts", &binary.path))
        .num_columns(2)
        .spacing([12.0, 4.0]);
    facts = facts.min_col_width(80.0);
    facts.show(ui, |ui| {
        if !binary.libraries.is_empty() {
            row(ui, "Links against", binary.libraries.join(", "));
        }
        for (category, names) in &binary.capabilities {
            row(ui, category.label(), names.join(", "));
        }
    });
}

/// `PASS`, `WEAK`, `FAIL` or `?`, colored.
fn status_badge(status: Status) -> RichText {
    let (text, color) = match status {
        Status::Good => ("PASS", GOOD_COLOR),
        Status::Weak => ("WEAK", WEAK_COLOR),
        Status::Bad => ("FAIL", ERROR_COLOR),
        Status::Unknown => ("?", Color32::GRAY),
    };
    RichText::new(text).color(color).strong().monospace()
}
