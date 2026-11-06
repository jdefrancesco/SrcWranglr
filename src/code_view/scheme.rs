//! Source color schemes: the code view's background, text, syntax and
//! highlight colors, picked in Settings.

use egui::Color32;

/// A named set of source colors.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scheme {
    #[default]
    Classic,
    SolarizedDark,
    SolarizedLight,
    Monokai,
    GruvboxDark,
    PaperLight,
}

/// Every color the code view draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceColors {
    pub background: Color32,
    pub text: Color32,
    pub keyword: Color32,
    pub string: Color32,
    pub comment: Color32,
    pub number: Color32,
    pub preproc: Color32,
    pub line_number: Color32,
    /// Behind the line in focus.
    pub active_line: Color32,
    /// Behind the name in focus (a step of a trace, a clicked location).
    pub active_name: Color32,
    /// Behind the other occurrences of the selected symbol.
    pub selected_name: Color32,
}

impl Default for SourceColors {
    fn default() -> Self {
        Scheme::default().colors()
    }
}

const fn rgb(hex: u32) -> Color32 {
    let [_, r, g, b] = hex.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

impl Scheme {
    pub const ALL: [Scheme; 6] = [
        Scheme::Classic,
        Scheme::SolarizedDark,
        Scheme::SolarizedLight,
        Scheme::Monokai,
        Scheme::GruvboxDark,
        Scheme::PaperLight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Scheme::Classic => "SrcWranglr Dark",
            Scheme::SolarizedDark => "Solarized Dark",
            Scheme::SolarizedLight => "Solarized Light",
            Scheme::Monokai => "Monokai",
            Scheme::GruvboxDark => "Gruvbox Dark",
            Scheme::PaperLight => "Paper Light",
        }
    }

    /// How the scheme is written in the settings file.
    pub fn id(self) -> &'static str {
        match self {
            Scheme::Classic => "srcwranglr-dark",
            Scheme::SolarizedDark => "solarized-dark",
            Scheme::SolarizedLight => "solarized-light",
            Scheme::Monokai => "monokai",
            Scheme::GruvboxDark => "gruvbox-dark",
            Scheme::PaperLight => "paper-light",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    pub fn colors(self) -> SourceColors {
        match self {
            Scheme::Classic => CLASSIC,
            Scheme::SolarizedDark => SOLARIZED_DARK,
            Scheme::SolarizedLight => SOLARIZED_LIGHT,
            Scheme::Monokai => MONOKAI,
            Scheme::GruvboxDark => GRUVBOX_DARK,
            Scheme::PaperLight => PAPER_LIGHT,
        }
    }
}

const CLASSIC: SourceColors = SourceColors {
    background: Color32::from_gray(18),
    text: Color32::from_gray(210),
    keyword: rgb(0xCC7AD9),
    string: rgb(0x9EC96B),
    comment: rgb(0x6A6A6A),
    number: rgb(0xD9A64A),
    preproc: rgb(0x6A9ED9),
    line_number: Color32::from_gray(90),
    active_line: rgb(0x3A3A14),
    active_name: rgb(0x8A6D1C),
    selected_name: rgb(0x2E4563),
};

const SOLARIZED_DARK: SourceColors = SourceColors {
    background: rgb(0x002B36),
    text: rgb(0x93A1A1),
    keyword: rgb(0x859900),
    string: rgb(0x2AA198),
    comment: rgb(0x657B83),
    number: rgb(0xD33682),
    preproc: rgb(0xCB4B16),
    line_number: rgb(0x586E75),
    active_line: rgb(0x073642),
    active_name: rgb(0x584A00),
    selected_name: rgb(0x1E4A6B),
};

const SOLARIZED_LIGHT: SourceColors = SourceColors {
    background: rgb(0xFDF6E3),
    text: rgb(0x586E75),
    keyword: rgb(0x6C7C00),
    string: rgb(0x1E7C74),
    comment: rgb(0x839496),
    number: rgb(0xB8286C),
    preproc: rgb(0xB0400F),
    line_number: rgb(0x93A1A1),
    active_line: rgb(0xEEE8D5),
    active_name: rgb(0xF2D98A),
    selected_name: rgb(0xD6E4EE),
};

const MONOKAI: SourceColors = SourceColors {
    background: rgb(0x272822),
    text: rgb(0xF8F8F2),
    keyword: rgb(0xF92672),
    string: rgb(0xE6DB74),
    comment: rgb(0x8C8770),
    number: rgb(0xAE81FF),
    preproc: rgb(0x66D9EF),
    line_number: rgb(0x90908A),
    active_line: rgb(0x3E3D32),
    active_name: rgb(0x75621A),
    selected_name: rgb(0x3A4A63),
};

const GRUVBOX_DARK: SourceColors = SourceColors {
    background: rgb(0x282828),
    text: rgb(0xEBDBB2),
    keyword: rgb(0xFB4934),
    string: rgb(0xB8BB26),
    comment: rgb(0x928374),
    number: rgb(0xD3869B),
    preproc: rgb(0x8EC07C),
    line_number: rgb(0x7C6F64),
    active_line: rgb(0x3C3836),
    active_name: rgb(0x79740E),
    selected_name: rgb(0x2B4F5C),
};

const PAPER_LIGHT: SourceColors = SourceColors {
    background: rgb(0xFFFFFF),
    text: rgb(0x24292F),
    keyword: rgb(0xCF222E),
    string: rgb(0x0A3069),
    comment: rgb(0x6E7781),
    number: rgb(0x0550AE),
    preproc: rgb(0x8250DF),
    line_number: rgb(0x8C959F),
    active_line: rgb(0xFFF8C5),
    active_name: rgb(0xF2CC60),
    selected_name: rgb(0xDDF4FF),
};

/// Code shown to preview a scheme.
const SAMPLE: &[&str] = &[
    "#include <string.h>",
    "// Copy the name the user typed.",
    "int copy_name(char *dst, const char *src) {",
    "    strcpy(dst, \"guest\");",
    "    return 42;",
    "}",
];

/// Draws a few lines of code in `colors`, line numbers and the line in
/// focus included, to preview a scheme.
pub fn show_sample(ui: &mut egui::Ui, colors: &SourceColors) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    egui::Frame::NONE
        .fill(colors.background)
        .inner_margin(6.0)
        .show(ui, |ui| {
            for (index, line) in SAMPLE.iter().enumerate() {
                let mut job = egui::text::LayoutJob::default();
                let number = egui::TextFormat::simple(font.clone(), colors.line_number);
                job.append(&format!("{:>3}  ", index + 1), 0.0, number);
                for (range, color) in super::tokenize::colored_ranges(line, colors) {
                    let format = egui::TextFormat {
                        background: match index {
                            3 => colors.active_line,
                            _ => Color32::TRANSPARENT,
                        },
                        ..egui::TextFormat::simple(font.clone(), color)
                    };
                    job.append(&line[range], 0.0, format);
                }
                ui.label(job);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn luminance(c: Color32) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn every_scheme_keeps_code_readable() {
        for scheme in Scheme::ALL {
            let c = scheme.colors();
            let check = |what: &str, color: Color32, on: Color32, least: f64| {
                let ratio = contrast(color, on);
                assert!(ratio >= least, "{}: {what} is {ratio:.2}:1", scheme.label());
            };
            check("text", c.text, c.background, 4.5);
            for (what, color) in [
                ("keyword", c.keyword),
                ("string", c.string),
                ("number", c.number),
                ("preproc", c.preproc),
            ] {
                check(what, color, c.background, 3.0);
            }
            check("comment", c.comment, c.background, 2.5);
            check("line number", c.line_number, c.background, 2.0);
            check("text on the name in focus", c.text, c.active_name, 3.0);
            check("text on the selected name", c.text, c.selected_name, 3.0);
            check("text on the line in focus", c.text, c.active_line, 4.0);
        }
    }

    #[test]
    fn ids_round_trip() {
        for scheme in Scheme::ALL {
            assert_eq!(Scheme::from_id(scheme.id()), Some(scheme));
        }
        assert_eq!(Scheme::from_id("nope"), None);
        assert_eq!(
            Scheme::default().colors().background,
            Color32::from_gray(18)
        );
    }
}
