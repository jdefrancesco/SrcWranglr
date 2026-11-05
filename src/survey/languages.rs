//! Which languages a file is in, by name or extension, and how many of its
//! lines are code, comments, or blank. Comment detection is line-based and
//! approximate (like `cloc`), which is plenty for sizing up a codebase.

use std::path::Path;

/// How a language writes comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comments {
    /// `//` to end of line and `/* … */` blocks.
    CStyle,
    /// `#` to end of line.
    Hash,
    /// `--` to end of line.
    DoubleDash,
    /// Assembly: `;`, `#`, `@` or `//` to end of line.
    Assembly,
    /// No comments recognized: lines are code or blank.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Code,
    Build,
    Docs,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    pub name: &'static str,
    pub kind: Kind,
    pub comments: Comments,
}

const fn lang(name: &'static str, kind: Kind, comments: Comments) -> Language {
    Language {
        name,
        kind,
        comments,
    }
}

const C: Language = lang("C", Kind::Code, Comments::CStyle);
const CPP: Language = lang("C++", Kind::Code, Comments::CStyle);
const HEADER: Language = lang("C/C++ header", Kind::Code, Comments::CStyle);
const ASM: Language = lang("Assembly", Kind::Code, Comments::Assembly);
const MAKE: Language = lang("Make", Kind::Build, Comments::Hash);
const CMAKE: Language = lang("CMake", Kind::Build, Comments::Hash);

/// Languages by (lowercase) file extension.
const BY_EXTENSION: &[(&str, Language)] = &[
    ("c", C),
    ("cc", CPP),
    ("cpp", CPP),
    ("cxx", CPP),
    ("c++", CPP),
    ("h", HEADER),
    ("hh", HEADER),
    ("hpp", HEADER),
    ("hxx", HEADER),
    ("h++", HEADER),
    ("inl", HEADER),
    ("ipp", HEADER),
    ("tpp", HEADER),
    ("m", lang("Objective-C", Kind::Code, Comments::CStyle)),
    ("mm", lang("Objective-C++", Kind::Code, Comments::CStyle)),
    ("cu", lang("CUDA", Kind::Code, Comments::CStyle)),
    ("cuh", lang("CUDA", Kind::Code, Comments::CStyle)),
    ("s", ASM),
    ("asm", ASM),
    ("rs", lang("Rust", Kind::Code, Comments::CStyle)),
    ("go", lang("Go", Kind::Code, Comments::CStyle)),
    ("java", lang("Java", Kind::Code, Comments::CStyle)),
    ("kt", lang("Kotlin", Kind::Code, Comments::CStyle)),
    ("cs", lang("C#", Kind::Code, Comments::CStyle)),
    ("swift", lang("Swift", Kind::Code, Comments::CStyle)),
    ("js", lang("JavaScript", Kind::Code, Comments::CStyle)),
    ("mjs", lang("JavaScript", Kind::Code, Comments::CStyle)),
    ("ts", lang("TypeScript", Kind::Code, Comments::CStyle)),
    ("tsx", lang("TypeScript", Kind::Code, Comments::CStyle)),
    ("zig", lang("Zig", Kind::Code, Comments::CStyle)),
    ("d", lang("D", Kind::Code, Comments::CStyle)),
    ("py", lang("Python", Kind::Code, Comments::Hash)),
    ("rb", lang("Ruby", Kind::Code, Comments::Hash)),
    ("pl", lang("Perl", Kind::Code, Comments::Hash)),
    ("pm", lang("Perl", Kind::Code, Comments::Hash)),
    ("sh", lang("Shell", Kind::Code, Comments::Hash)),
    ("bash", lang("Shell", Kind::Code, Comments::Hash)),
    ("zsh", lang("Shell", Kind::Code, Comments::Hash)),
    ("ps1", lang("PowerShell", Kind::Code, Comments::Hash)),
    ("php", lang("PHP", Kind::Code, Comments::CStyle)),
    ("lua", lang("Lua", Kind::Code, Comments::DoubleDash)),
    ("sql", lang("SQL", Kind::Code, Comments::DoubleDash)),
    ("hs", lang("Haskell", Kind::Code, Comments::DoubleDash)),
    ("bat", lang("Batch", Kind::Code, Comments::None)),
    ("cmd", lang("Batch", Kind::Code, Comments::None)),
    (
        "proto",
        lang("Protocol Buffers", Kind::Code, Comments::CStyle),
    ),
    ("cmake", CMAKE),
    ("mk", MAKE),
    ("mak", MAKE),
    ("bzl", lang("Bazel", Kind::Build, Comments::Hash)),
    ("m4", lang("Autoconf", Kind::Build, Comments::Hash)),
    ("ac", lang("Autoconf", Kind::Build, Comments::Hash)),
    ("am", lang("Automake", Kind::Build, Comments::Hash)),
    ("vcxproj", lang("MSBuild", Kind::Build, Comments::None)),
    ("gradle", lang("Gradle", Kind::Build, Comments::CStyle)),
    ("md", lang("Markdown", Kind::Docs, Comments::None)),
    ("rst", lang("reStructuredText", Kind::Docs, Comments::None)),
    ("txt", lang("Text", Kind::Docs, Comments::None)),
    ("html", lang("HTML", Kind::Docs, Comments::None)),
    ("json", lang("JSON", Kind::Data, Comments::None)),
    ("yaml", lang("YAML", Kind::Data, Comments::Hash)),
    ("yml", lang("YAML", Kind::Data, Comments::Hash)),
    ("toml", lang("TOML", Kind::Data, Comments::Hash)),
    ("xml", lang("XML", Kind::Data, Comments::None)),
    ("ini", lang("INI", Kind::Data, Comments::None)),
];

/// Languages by exact file name (checked before the extension).
const BY_FILE_NAME: &[(&str, Language)] = &[
    ("CMakeLists.txt", CMAKE),
    ("Makefile", MAKE),
    ("makefile", MAKE),
    ("GNUmakefile", MAKE),
    ("Kbuild", MAKE),
    ("meson.build", lang("Meson", Kind::Build, Comments::Hash)),
    ("BUILD", lang("Bazel", Kind::Build, Comments::Hash)),
    ("BUILD.bazel", lang("Bazel", Kind::Build, Comments::Hash)),
    ("SConstruct", lang("SCons", Kind::Build, Comments::Hash)),
    ("SConscript", lang("SCons", Kind::Build, Comments::Hash)),
    ("Kconfig", lang("Kconfig", Kind::Build, Comments::Hash)),
    (
        "Dockerfile",
        lang("Dockerfile", Kind::Build, Comments::Hash),
    ),
];

/// The language of `path`, if it is one we count.
pub fn language_of(path: &Path) -> Option<Language> {
    let name = path.file_name()?.to_str()?;
    if let Some((_, language)) = BY_FILE_NAME.iter().find(|(n, _)| *n == name) {
        return Some(*language);
    }
    // `.S` is assembly run through the C preprocessor; same counting.
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    BY_EXTENSION
        .iter()
        .find(|(e, _)| *e == extension)
        .map(|(_, language)| *language)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineCounts {
    pub code: u64,
    pub comments: u64,
    pub blank: u64,
}

impl LineCounts {
    pub fn plus(self, other: LineCounts) -> LineCounts {
        LineCounts {
            code: self.code + other.code,
            comments: self.comments + other.comments,
            blank: self.blank + other.blank,
        }
    }
}

/// Counts `text`'s code, comment, and blank lines.
pub fn count_lines(text: &str, comments: Comments) -> LineCounts {
    let mut counts = LineCounts::default();
    let mut is_in_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        let (has_code, has_comment) = match comments {
            Comments::CStyle => c_style_line(trimmed, &mut is_in_block),
            Comments::Hash => prefixed_line(trimmed, &["#"]),
            Comments::DoubleDash => prefixed_line(trimmed, &["--"]),
            Comments::Assembly => prefixed_line(trimmed, &[";", "#", "@", "//"]),
            Comments::None => (!trimmed.is_empty(), false),
        };
        if has_code {
            counts.code += 1;
        } else if has_comment {
            counts.comments += 1;
        } else {
            counts.blank += 1;
        }
    }
    counts
}

fn prefixed_line(trimmed: &str, prefixes: &[&str]) -> (bool, bool) {
    if trimmed.is_empty() {
        return (false, false);
    }
    let is_comment = prefixes.iter().any(|p| trimmed.starts_with(p));
    (!is_comment, is_comment)
}

/// Whether a line has code and/or comment text, tracking `/* … */`
/// across lines. String literals are skipped so `"/*"` isn't a comment.
fn c_style_line(trimmed: &str, is_in_block: &mut bool) -> (bool, bool) {
    let bytes = trimmed.as_bytes();
    let (mut has_code, mut has_comment) = (false, *is_in_block && !trimmed.is_empty());
    let mut i = 0;
    while i < bytes.len() {
        if *is_in_block {
            has_comment = true;
            match trimmed[i..].find("*/") {
                Some(end) => {
                    *is_in_block = false;
                    i += end + 2;
                }
                None => return (has_code, true),
            }
            continue;
        }
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => return (has_code, true),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                *is_in_block = true;
                has_comment = true;
                i += 2;
            }
            b'"' | b'\'' => {
                has_code = true;
                i = skip_literal(bytes, i);
            }
            b' ' | b'\t' => i += 1,
            _ => {
                has_code = true;
                i += 1;
            }
        }
    }
    (has_code, has_comment)
}

/// The index just past the string or character literal starting at `start`.
fn skip_literal(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(code: u64, comments: u64, blank: u64) -> LineCounts {
        LineCounts {
            code,
            comments,
            blank,
        }
    }

    #[test]
    fn knows_languages_by_extension_and_file_name() {
        let name = |p: &str| language_of(Path::new(p)).map(|l| l.name);
        assert_eq!(name("src/a.c"), Some("C"));
        assert_eq!(name("src/a.CPP"), Some("C++"));
        assert_eq!(name("include/a.h"), Some("C/C++ header"));
        assert_eq!(name("boot/start.S"), Some("Assembly"));
        assert_eq!(name("CMakeLists.txt"), Some("CMake"));
        assert_eq!(name("sub/Makefile"), Some("Make"));
        assert_eq!(name("notes.txt"), Some("Text"));
        assert_eq!(name("image.png"), None);
        assert_eq!(name("noextension"), None);
    }

    #[test]
    fn counts_c_code_comments_and_blank_lines() {
        let text = "#include <x.h>\n\n// line comment\n/* block\n   still */\nint a; /* tail */\n/* a */ int b;\n  \n";
        assert_eq!(count_lines(text, Comments::CStyle), counts(3, 3, 2));
    }

    #[test]
    fn a_comment_marker_inside_a_string_is_code() {
        let text = "const char *s = \"/* not a comment\";\nint after;\n";
        assert_eq!(count_lines(text, Comments::CStyle), counts(2, 0, 0));
        let escaped = "char q = '\\''; // quote\n";
        assert_eq!(count_lines(escaped, Comments::CStyle), counts(1, 0, 0));
    }

    #[test]
    fn a_block_comment_opened_after_code_continues() {
        let text = "x = 1; /* starts\n  ends */\ny = 2;\n";
        assert_eq!(count_lines(text, Comments::CStyle), counts(2, 1, 0));
    }

    #[test]
    fn counts_hash_dash_and_assembly_comments() {
        assert_eq!(
            count_lines("# c\nx = 1\n\n", Comments::Hash),
            counts(1, 1, 1)
        );
        assert_eq!(
            count_lines("-- c\nlocal x\n", Comments::DoubleDash),
            counts(1, 1, 0)
        );
        let asm = "; nasm\n# gas\n@ arm\nmov r0, r1\n";
        assert_eq!(count_lines(asm, Comments::Assembly), counts(1, 3, 0));
        assert_eq!(count_lines("{}\n\n", Comments::None), counts(1, 0, 1));
    }

    #[test]
    fn adds_counts_up() {
        assert_eq!(counts(1, 2, 3).plus(counts(10, 20, 30)), counts(11, 22, 33));
    }
}
