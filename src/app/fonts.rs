//! The fonts the app adds to egui's own: Ubuntu Regular in place of egui's
//! Ubuntu Light, so text reads heavier; Ubuntu Medium, for the pane
//! toggles; Source Code Pro and Iosevka, for the symbol list (picked in
//! Settings). Iosevka is also the last fallback everywhere, for the stars,
//! arrows and shapes egui's fonts lack (☆, ▸).

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// Font names (in egui's font data) and family names (for `SymbolFont`).
const SOURCE_CODE_PRO: &str = "Source Code Pro";
const IOSEVKA: &str = "Iosevka";
const SOURCE_CODE_PRO_TTF: &[u8] = include_bytes!("../../assets/fonts/SourceCodePro-Regular.ttf");
/// A subset: Latin, punctuation, arrows, math, shapes (see assets/fonts).
const IOSEVKA_TTF: &[u8] = include_bytes!("../../assets/fonts/Iosevka-Regular.ttf");
/// egui's text font, too thin to read easily; Ubuntu Regular replaces it.
const EGUI_UBUNTU_LIGHT: &str = "Ubuntu-Light";
const UBUNTU_REGULAR: &str = "Ubuntu-Regular";
const UBUNTU_MEDIUM: &str = "Ubuntu-Medium";
const UBUNTU_REGULAR_TTF: &[u8] = include_bytes!("../../assets/fonts/Ubuntu-Regular.ttf");
const UBUNTU_MEDIUM_TTF: &[u8] = include_bytes!("../../assets/fonts/Ubuntu-Medium.ttf");

/// The font the symbol list is drawn in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SymbolFont {
    /// Hack, egui's monospace font, as in the source pane.
    #[default]
    Monospace,
    SourceCodePro,
    Iosevka,
}

impl SymbolFont {
    pub const ALL: [SymbolFont; 3] = [
        SymbolFont::Monospace,
        SymbolFont::SourceCodePro,
        SymbolFont::Iosevka,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SymbolFont::Monospace => "Monospace (Hack)",
            SymbolFont::SourceCodePro => "Source Code Pro",
            SymbolFont::Iosevka => "Iosevka",
        }
    }

    /// The name it is saved as in the settings file.
    pub fn id(self) -> &'static str {
        match self {
            SymbolFont::Monospace => "monospace",
            SymbolFont::SourceCodePro => "source-code-pro",
            SymbolFont::Iosevka => "iosevka",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|font| font.id() == id)
    }

    /// The egui font family to draw text in this font.
    pub fn family(self) -> FontFamily {
        match self {
            SymbolFont::Monospace => FontFamily::Monospace,
            SymbolFont::SourceCodePro => FontFamily::Name(SOURCE_CODE_PRO.into()),
            SymbolFont::Iosevka => FontFamily::Name(IOSEVKA.into()),
        }
    }
}

/// The family for text that should stand out a little from the rest, such
/// as the pane toggles: Ubuntu Medium, then the proportional fonts.
pub fn medium() -> FontFamily {
    FontFamily::Name(UBUNTU_MEDIUM.into())
}

/// egui's fonts plus ours: Ubuntu Regular wherever egui uses Ubuntu Light,
/// a family per bundled font (falling back to the monospace family for
/// what it lacks), and Iosevka last in every family, so it only draws what
/// no other font can.
pub fn definitions() -> FontDefinitions {
    let defaults = FontDefinitions::default();
    let heavier = |name: &String| match name.as_str() {
        EGUI_UBUNTU_LIGHT => UBUNTU_REGULAR.to_string(),
        _ => name.clone(),
    };
    let with_iosevka_last = |fonts: &[String]| -> Vec<String> {
        let others = fonts.iter().filter(|name| *name != IOSEVKA).map(heavier);
        others.chain([IOSEVKA.to_string()]).collect()
    };
    let monospace = with_iosevka_last(
        defaults
            .families
            .get(&FontFamily::Monospace)
            .map_or(&[][..], Vec::as_slice),
    );
    let first_then_monospace = |name: &str| -> Vec<String> {
        let rest = monospace.iter().filter(|other| *other != name).cloned();
        [name.to_string()].into_iter().chain(rest).collect()
    };
    let proportional = with_iosevka_last(
        defaults
            .families
            .get(&FontFamily::Proportional)
            .map_or(&[][..], Vec::as_slice),
    );
    let medium_family = [UBUNTU_MEDIUM.to_string()]
        .into_iter()
        .chain(proportional)
        .collect();
    let builtin = defaults
        .families
        .iter()
        .map(|(family, fonts)| (family.clone(), with_iosevka_last(fonts)));
    let bundled = [SOURCE_CODE_PRO, IOSEVKA]
        .into_iter()
        .map(|name| (FontFamily::Name(name.into()), first_then_monospace(name)));
    let font_data = defaults
        .font_data
        .into_iter()
        .filter(|(name, _)| name != EGUI_UBUNTU_LIGHT)
        .chain(
            [
                (SOURCE_CODE_PRO, SOURCE_CODE_PRO_TTF),
                (IOSEVKA, IOSEVKA_TTF),
                (UBUNTU_REGULAR, UBUNTU_REGULAR_TTF),
                (UBUNTU_MEDIUM, UBUNTU_MEDIUM_TTF),
            ]
            .map(|(name, ttf)| (name.to_string(), Arc::new(FontData::from_static(ttf)))),
        );
    FontDefinitions {
        font_data: font_data.collect(),
        families: builtin
            .chain(bundled)
            .chain([(medium(), medium_family)])
            .collect(),
    }
}

/// Makes the fonts available to everything drawn with `ctx`.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

/// A test harness with the fonts the app has, so text the app can draw
/// doesn't fail the harness's check for missing glyphs.
#[cfg(test)]
pub fn test_harness<State>() -> egui_kittest::HarnessBuilder<State> {
    egui_kittest::Harness::builder().with_context_setup(install)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context with the fonts installed and loaded (egui loads them at
    /// the start of a frame).
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        // No renderer here to upload the font atlas to.
        output.textures_delta.clear();
        ctx
    }

    /// Which fonts of `family` draw `c`, first choice first.
    fn fonts_drawing(ctx: &egui::Context, family: &FontFamily, c: char) -> Vec<String> {
        ctx.fonts_mut(|fonts| fonts.characters(family).get(&c).cloned())
            .unwrap_or_default()
    }

    #[test]
    fn each_symbol_font_draws_text_in_its_own_typeface() {
        let ctx = context();
        for (font, name) in [
            (SymbolFont::Monospace, "Hack"),
            (SymbolFont::SourceCodePro, "Source Code Pro"),
            (SymbolFont::Iosevka, "Iosevka"),
        ] {
            let drawn_by = fonts_drawing(&ctx, &font.family(), 'g');
            assert_eq!(drawn_by.first().map(String::as_str), Some(name), "{font:?}");
        }
    }

    #[test]
    fn the_ui_can_draw_the_stars_and_arrows_the_panes_use() {
        let ctx = context();
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            for c in ['☆', '★', '▸', '◂', '▾', '•', '×'] {
                let drawn_by = fonts_drawing(&ctx, &family, c);
                assert!(!drawn_by.is_empty(), "{family:?} can't draw {c}");
            }
        }
    }

    #[test]
    fn our_fallback_does_not_change_text_ubuntu_already_draws() {
        let ctx = context();
        let drawn_by = fonts_drawing(&ctx, &FontFamily::Proportional, 'a');
        assert_eq!(drawn_by.first().map(String::as_str), Some(UBUNTU_REGULAR));
        let bullet = fonts_drawing(&ctx, &FontFamily::Proportional, '•');
        assert_eq!(bullet.first().map(String::as_str), Some(UBUNTU_REGULAR));
    }

    #[test]
    fn text_is_drawn_in_ubuntu_regular_instead_of_egui_s_thin_light() {
        let definitions = definitions();
        assert!(!definitions.font_data.contains_key(EGUI_UBUNTU_LIGHT));
        for fonts in definitions.families.values() {
            assert!(
                !fonts.iter().any(|name| name == EGUI_UBUNTU_LIGHT),
                "{fonts:?}"
            );
        }
        let ctx = context();
        let mono_fallback = fonts_drawing(&ctx, &FontFamily::Monospace, 'é');
        assert!(!mono_fallback.is_empty());
    }

    #[test]
    fn the_medium_family_draws_in_ubuntu_medium_and_falls_back() {
        let ctx = context();
        let drawn_by = fonts_drawing(&ctx, &medium(), 'T');
        assert_eq!(drawn_by.first().map(String::as_str), Some(UBUNTU_MEDIUM));
        assert!(!fonts_drawing(&ctx, &medium(), '▸').is_empty());
    }

    #[test]
    fn symbol_fonts_are_saved_by_id_and_read_back() {
        for font in SymbolFont::ALL {
            assert_eq!(SymbolFont::from_id(font.id()), Some(font));
        }
        assert_eq!(SymbolFont::from_id("comic-sans"), None);
    }
}
