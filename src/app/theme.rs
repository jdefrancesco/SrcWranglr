//! The app's colors: egui's own light and dark themes with more contrast.
//! egui draws text mid-gray and faint text at 60% of that, which is hard
//! to read on the light theme's near-white panels; here text is near-black
//! (near-white on dark), faint text is a solid gray that still reads, and
//! separators and button backgrounds are a shade stronger.

use egui::{Color32, Stroke, Theme, Visuals};

/// Makes both themes available to everything drawn with `ctx`; which one
/// shows still follows the system (or the user's choice).
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Light, light());
    ctx.set_visuals_of(Theme::Dark, dark());
}

/// The colors each theme changes from egui's.
struct Contrast {
    text: Color32,
    button_text: Color32,
    weak_text: Color32,
    separator: Color32,
    button_fill: Option<Color32>,
    hyperlink: Color32,
}

const LIGHT: Contrast = Contrast {
    text: Color32::from_gray(28),
    button_text: Color32::from_gray(20),
    // About 5.5:1 against the panels, where egui's faint text is under 2:1.
    weak_text: Color32::from_gray(100),
    separator: Color32::from_gray(170),
    button_fill: Some(Color32::from_gray(222)),
    hyperlink: Color32::from_rgb(0, 102, 204),
};

const DARK: Contrast = Contrast {
    text: Color32::from_gray(212),
    button_text: Color32::from_gray(232),
    weak_text: Color32::from_gray(150),
    separator: Color32::from_gray(75),
    button_fill: None,
    hyperlink: Color32::from_rgb(90, 170, 255),
};

pub fn light() -> Visuals {
    with_contrast(Visuals::light(), &LIGHT)
}

pub fn dark() -> Visuals {
    with_contrast(Visuals::dark(), &DARK)
}

fn with_contrast(base: Visuals, contrast: &Contrast) -> Visuals {
    let widgets = base.widgets.clone();
    let noninteractive = egui::style::WidgetVisuals {
        fg_stroke: Stroke::new(widgets.noninteractive.fg_stroke.width, contrast.text),
        bg_stroke: Stroke::new(widgets.noninteractive.bg_stroke.width, contrast.separator),
        ..widgets.noninteractive
    };
    let button_fill = contrast.button_fill;
    let inactive = egui::style::WidgetVisuals {
        fg_stroke: Stroke::new(widgets.inactive.fg_stroke.width, contrast.button_text),
        weak_bg_fill: button_fill.unwrap_or(widgets.inactive.weak_bg_fill),
        bg_fill: button_fill.unwrap_or(widgets.inactive.bg_fill),
        ..widgets.inactive
    };
    Visuals {
        widgets: egui::style::Widgets {
            noninteractive,
            inactive,
            ..widgets
        },
        weak_text_color: Some(contrast.weak_text),
        hyperlink_color: contrast.hyperlink,
        ..base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance of an opaque color.
    fn luminance(color: Color32) -> f32 {
        let channel = |c: u8| {
            let c = f32::from(c) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (hi, lo) = {
            let (la, lb) = (luminance(a), luminance(b));
            if la > lb {
                (la, lb)
            } else {
                (lb, la)
            }
        };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn text_and_faint_text_read_clearly_on_both_themes() {
        for visuals in [light(), dark()] {
            let panel = visuals.panel_fill;
            let text = contrast(visuals.text_color(), panel);
            let weak = contrast(visuals.weak_text_color(), panel);
            assert!(text >= 10.0, "text {text} (dark: {})", visuals.dark_mode);
            assert!(
                weak >= 4.5,
                "faint text {weak} (dark: {})",
                visuals.dark_mode
            );
            let button = &visuals.widgets.inactive;
            let on_button = contrast(button.fg_stroke.color, button.weak_bg_fill);
            assert!(on_button >= 7.0, "button text {on_button}");
            let link = contrast(visuals.hyperlink_color, panel);
            assert!(link >= 4.5, "links {link} (dark: {})", visuals.dark_mode);
        }
    }

    #[test]
    fn faint_text_is_still_fainter_than_text() {
        for visuals in [light(), dark()] {
            let panel = visuals.panel_fill;
            assert!(
                contrast(visuals.weak_text_color(), panel) < contrast(visuals.text_color(), panel)
            );
        }
    }

    #[test]
    fn each_theme_keeps_its_own_background() {
        assert!(!light().dark_mode);
        assert!(dark().dark_mode);
        assert_eq!(light().panel_fill, Visuals::light().panel_fill);
        assert_eq!(dark().panel_fill, Visuals::dark().panel_fill);
    }
}
