//! The Project Triage tab listing functions worth fuzzing, best first,
//! each with why it scored and a way to draft its harness.

use egui::{Color32, RichText};

use super::survey_view::{plural, thousands, SurveyAction};
use crate::analysis::fuzz::{Ease, FuzzTarget, Reason};

const EASY_COLOR: Color32 = Color32::from_rgb(0x6C, 0xC0, 0x7A);
const MODERATE_COLOR: Color32 = Color32::from_rgb(0xD9, 0xA6, 0x4A);
const HARD_COLOR: Color32 = Color32::from_rgb(0xE0, 0x6C, 0x6C);
const ROW_HEIGHT: f32 = 20.0;
const SCORE_WIDTH: f32 = 36.0;
const EASE_WIDTH: f32 = 64.0;

/// `12 functions worth fuzzing (5 easy to harness); best: parse_chunk`.
pub fn fuzz_summary(targets: &[FuzzTarget]) -> String {
    let Some(best) = targets.first() else {
        return "no functions stand out as fuzz targets".into();
    };
    let easy = targets.iter().filter(|t| t.ease == Ease::Easy).count();
    format!(
        "{} {} worth fuzzing ({easy} easy to harness); best: {}",
        thousands(targets.len() as u64),
        plural(targets.len(), "function"),
        best.name
    )
}

pub fn fuzz_tab(
    ui: &mut egui::Ui,
    targets: &[FuzzTarget],
    query: &mut String,
) -> Option<SurveyAction> {
    ui.label(fuzz_summary(targets));
    ui.weak("Worth: a name that says it takes input apart, risky calls inside, many paths, a caller that reads input. Ease: a buffer and its length is best; values built by hand, needing an object, or being static cost. Hover a row for its reasons; Harness… drafts a libFuzzer harness to review and save.");
    ui.horizontal(|ui| {
        let label = ui.label("Filter");
        ui.add(
            egui::TextEdit::singleline(query)
                .hint_text("function or file")
                .desired_width(240.0),
        )
        .labelled_by(label.id);
    });
    let needle = query.trim().to_lowercase();
    let shown: Vec<&FuzzTarget> = targets
        .iter()
        .filter(|t| {
            needle.is_empty()
                || t.name.to_lowercase().contains(&needle)
                || t.file.to_lowercase().contains(&needle)
        })
        .collect();
    ui.separator();
    let mut action = None;
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(
        ui,
        ROW_HEIGHT,
        shown.len(),
        |ui, range| {
            for target in &shown[range] {
                if let Some(clicked) = target_row(ui, target) {
                    action = Some(clicked);
                }
            }
        },
    );
    action
}

/// Draws one target; returns what was clicked.
fn target_row(ui: &mut egui::Ui, target: &FuzzTarget) -> Option<SurveyAction> {
    let row = ui.horizontal(|ui| {
        let color = ease_color(target.ease);
        let score = RichText::new(target.score.to_string())
            .color(color)
            .monospace()
            .strong();
        ui.add_sized([SCORE_WIDTH, ROW_HEIGHT], egui::Label::new(score));
        let ease = RichText::new(target.ease.label()).color(color);
        ui.add_sized([EASE_WIDTH, ROW_HEIGHT], egui::Label::new(ease));
        let go = ui.link(&target.name).clicked();
        let draft = ui.small_button("Harness…").clicked();
        ui.weak(format!("{}:{}", target.file, target.line));
        if let Some(top) = target.reasons.first() {
            ui.weak(&top.text);
        }
        if draft {
            Some(SurveyAction::Harness {
                symbol: target.symbol,
                file: target.file.clone(),
                line: target.line,
            })
        } else {
            go.then(|| SurveyAction::GoTo {
                symbol: Some(target.symbol),
                file: target.file.clone(),
                line: target.line,
            })
        }
    });
    row.response.on_hover_text(reasons_text(&target.reasons));
    row.inner
}

fn ease_color(ease: Ease) -> Color32 {
    match ease {
        Ease::Easy => EASY_COLOR,
        Ease::Moderate => MODERATE_COLOR,
        Ease::Hard => HARD_COLOR,
    }
}

/// `+3  its name says …` per line.
fn reasons_text(reasons: &[Reason]) -> String {
    reasons
        .iter()
        .map(|r| format!("{:+}  {}", r.points, r.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_are_listed_with_signed_points() {
        let reasons = [
            Reason {
                points: 3,
                text: "takes a string (s)".into(),
            },
            Reason {
                points: -2,
                text: "a method of P".into(),
            },
        ];
        assert_eq!(
            reasons_text(&reasons),
            "+3  takes a string (s)\n-2  a method of P"
        );
    }

    #[test]
    fn an_empty_list_says_nothing_stands_out() {
        assert_eq!(fuzz_summary(&[]), "no functions stand out as fuzz targets");
    }
}
