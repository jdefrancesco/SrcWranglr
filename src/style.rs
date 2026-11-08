//! Colors shared by several views.

use egui::Color32;

/// Bookmarks (starred notes), in the graph and the code gutter.
pub const BOOKMARK_COLOR: Color32 = Color32::from_rgb(0xE5, 0xB3, 0x3A);
/// Notes with text. Green, because the graph uses no green: blue would
/// vanish among "uses" wires and field pills.
pub const NOTE_COLOR: Color32 = Color32::from_rgb(0x4C, 0xC3, 0x8A);
/// Outline that keeps note/bookmark dots visible on pills and wires.
pub const MARK_OUTLINE: Color32 = Color32::from_gray(30);
/// Marks what outside input can reach.
pub const INPUT_COLOR: Color32 = Color32::from_rgb(0xE8, 0x8B, 0x3C);
/// The steps of a trace path, marked in the source.
pub const TRAIL_COLOR: Color32 = Color32::from_rgb(0x3C, 0xA8, 0xE8);

/// How a finding's severity is shown in the source pane, and on the dark
/// theme's panels.
pub fn severity_color(severity: crate::analysis::Severity) -> Color32 {
    match severity {
        crate::analysis::Severity::High => Color32::from_rgb(0xE0, 0x6C, 0x6C),
        crate::analysis::Severity::Medium => Color32::from_rgb(0xE0, 0x9B, 0x4A),
        crate::analysis::Severity::Low => Color32::from_gray(0x8A),
    }
}

/// A severity as text on a panel drawn with `visuals`: the same hues,
/// deeper on the light theme so they stand out from its near-white panels.
pub fn severity_text_color(
    severity: crate::analysis::Severity,
    visuals: &egui::Visuals,
) -> Color32 {
    if visuals.dark_mode {
        return severity_color(severity);
    }
    match severity {
        crate::analysis::Severity::High => Color32::from_rgb(0xC0, 0x30, 0x30),
        crate::analysis::Severity::Medium => Color32::from_rgb(0xB0, 0x5C, 0x00),
        crate::analysis::Severity::Low => Color32::from_gray(0x60),
    }
}
