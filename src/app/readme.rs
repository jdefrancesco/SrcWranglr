//! The project's README, for a quick read: the one at the top of the
//! project, read without following symlinks and only up to a size, then
//! split into headings, code and paragraphs to lay out. It comes from the
//! audited tree, so it is only ever shown as text, its hidden characters
//! marked and its size bounded: nothing in it is followed, fetched or run.

use std::fs;
use std::path::Path;

use super::matching::cut;
use super::visible::{visible, visible_lines};
use crate::safe_fs::read_regular_file;

/// A README bigger than this isn't read.
pub const MAX_README_BYTES: usize = 1024 * 1024;
/// Blocks past this many aren't shown, so a README of a million one-line
/// headings can't slow the window down.
const MAX_BLOCKS: usize = 2_000;
/// Each block's text is cut to this many characters.
const MAX_BLOCK_CHARS: usize = 10_000;

/// README file names, best first, matched regardless of case.
const NAMES: &[&str] = &[
    "readme.md",
    "readme.markdown",
    "readme",
    "readme.txt",
    "readme.rst",
    "readme.adoc",
    "readme.org",
];

/// File names read as Markdown; the others are shown as plain text.
const MARKDOWN_NAMES: &[&str] = &["readme.md", "readme.markdown"];

/// What the README window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readme {
    Found {
        file_name: String,
        /// Ready to show: hidden characters marked, each cut to size.
        blocks: Vec<Block>,
        /// More blocks than are shown.
        is_cut_short: bool,
    },
    /// A README is there but couldn't be read, and why.
    Unreadable { file_name: String, error: String },
    /// No README at the top of the project.
    Missing,
}

/// A piece of the README to lay out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A Markdown heading, level 1 to 6.
    Heading { level: u8, text: String },
    /// A fenced code block.
    Code(String),
    /// A paragraph, line breaks kept.
    Text(String),
}

/// The README at the top of `root`, read and split into blocks.
pub fn read(root: &Path) -> Readme {
    let file_name = match find(root) {
        Ok(Some(name)) => name,
        Ok(None) => return Readme::Missing,
        Err(error) => {
            return Readme::Unreadable {
                file_name: "the project folder".into(),
                error: error.to_string(),
            }
        }
    };
    match read_regular_file(&root.join(&file_name), MAX_README_BYTES) {
        Ok(Some(text)) => {
            let is_markdown = MARKDOWN_NAMES.contains(&file_name.to_lowercase().as_str());
            let (blocks, is_cut_short) = ready_to_show(blocks(&text, is_markdown));
            Readme::Found {
                file_name,
                blocks,
                is_cut_short,
            }
        }
        Ok(None) => Readme::Missing,
        Err(error) => Readme::Unreadable {
            file_name,
            error: error.to_string(),
        },
    }
}

/// The name of the best README directly in `root`, if any; folders don't
/// count, but symlinks do (to say they aren't followed). Of two names
/// equally good (`README.md` and `readme.md`), the first in byte order.
fn find(root: &Path) -> std::io::Result<Option<String>> {
    let best = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| !t.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| rank(&name).map(|rank| (rank, name)))
        .min();
    Ok(best.map(|(_, name)| name))
}

/// Where `name` stands in `NAMES`, if it is a README at all.
fn rank(name: &str) -> Option<usize> {
    let lower = name.to_lowercase();
    NAMES.iter().position(|&known| known == lower)
}

/// `blocks` ready to show: the first `MAX_BLOCKS`, each cut to
/// `MAX_BLOCK_CHARS` with its hidden characters marked; and whether any
/// were left out.
fn ready_to_show(blocks: Vec<Block>) -> (Vec<Block>, bool) {
    let is_cut_short = blocks.len() > MAX_BLOCKS;
    let shown = blocks
        .into_iter()
        .take(MAX_BLOCKS)
        .map(|block| match block {
            Block::Heading { level, text } => Block::Heading {
                level,
                text: visible(&cut(&text, MAX_BLOCK_CHARS)).into_owned(),
            },
            Block::Code(code) => Block::Code(visible_lines(&cut(&code, MAX_BLOCK_CHARS))),
            Block::Text(text) => Block::Text(visible_lines(&cut(&text, MAX_BLOCK_CHARS))),
        })
        .collect();
    (shown, is_cut_short)
}

/// `text` split into blocks: paragraphs, plus Markdown headings and fenced
/// code when `is_markdown`.
pub fn blocks(text: &str, is_markdown: bool) -> Vec<Block> {
    if !is_markdown {
        return paragraphs(&text.lines().collect::<Vec<_>>());
    }
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    let mut text_lines = Vec::new();
    while let Some(line) = lines.next() {
        if let Some(fence) = fence_of(line) {
            blocks.extend(paragraphs(&text_lines));
            text_lines.clear();
            let code: Vec<&str> = lines
                .by_ref()
                .take_while(|l| !l.trim_start().starts_with(fence))
                .collect();
            blocks.push(Block::Code(code.join("\n")));
        } else if let Some(heading) = heading_of(line) {
            blocks.extend(paragraphs(&text_lines));
            text_lines.clear();
            blocks.push(heading);
        } else {
            text_lines.push(line);
        }
    }
    blocks.extend(paragraphs(&text_lines));
    blocks
}

/// The fence a line opens a code block with: "```" or "~~~".
fn fence_of(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    ["```", "~~~"]
        .into_iter()
        .find(|fence| trimmed.starts_with(fence))
}

/// A `# Title` line as a heading: one to six `#`, then a space.
fn heading_of(line: &str) -> Option<Block> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let rest = &line[hashes..];
    let is_heading = (1..=6).contains(&hashes) && (rest.is_empty() || rest.starts_with(' '));
    is_heading.then(|| Block::Heading {
        level: hashes as u8,
        text: rest.trim().trim_end_matches('#').trim_end().to_string(),
    })
}

/// `lines` as paragraphs: runs of lines between blank ones.
fn paragraphs(lines: &[&str]) -> Vec<Block> {
    lines
        .split(|line| line.trim().is_empty())
        .filter(|run| !run.is_empty())
        .map(|run| Block::Text(run.join("\n")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Block {
        Block::Text(s.into())
    }

    fn heading(level: u8, s: &str) -> Block {
        Block::Heading {
            level,
            text: s.into(),
        }
    }

    #[test]
    fn markdown_splits_into_headings_code_and_text() {
        let readme = "# zlib\n\nA compression library.\nSmall and fast.\n\n## Build ##\n\
            ```sh\nmake\n# not a heading\n```\nThen install.\n#hashtag is text\n";
        assert_eq!(
            blocks(readme, true),
            [
                heading(1, "zlib"),
                text("A compression library.\nSmall and fast."),
                heading(2, "Build"),
                Block::Code("make\n# not a heading".into()),
                text("Then install.\n#hashtag is text"),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert_eq!(
            blocks("~~~\nint x;\n", true),
            [Block::Code("int x;".into())]
        );
    }

    #[test]
    fn plain_text_is_split_into_paragraphs_as_written() {
        let readme = "\nZLIB DATA COMPRESSION LIBRARY\nversion 1.3\n \n\n# not markdown\n";
        assert_eq!(
            blocks(readme, false),
            [
                text("ZLIB DATA COMPRESSION LIBRARY\nversion 1.3"),
                text("# not markdown")
            ]
        );
        assert!(blocks("\n \n", false).is_empty());
    }

    #[test]
    fn a_pathological_readme_is_cut_to_size() {
        let many_headings = "#\n".repeat(MAX_BLOCKS + 10);
        let (shown, is_cut_short) = ready_to_show(blocks(&many_headings, true));
        assert_eq!((shown.len(), is_cut_short), (MAX_BLOCKS, true));

        let one_long_line = "x".repeat(MAX_BLOCK_CHARS * 3);
        let (shown, is_cut_short) = ready_to_show(blocks(&one_long_line, true));
        let [Block::Text(text)] = shown.as_slice() else {
            panic!("{shown:?}");
        };
        assert_eq!(text.chars().count(), MAX_BLOCK_CHARS + 1);
        assert!(text.ends_with('…') && !is_cut_short);
    }

    #[test]
    fn hidden_characters_are_marked_once_when_read() {
        let (shown, _) = ready_to_show(blocks("# a\u{202e}b\n\nc\u{200b}d\n", true));
        let [Block::Heading { text: heading, .. }, Block::Text(body)] = shown.as_slice() else {
            panic!("{shown:?}");
        };
        assert!(!heading.contains('\u{202e}') && !body.contains('\u{200b}'));
    }

    #[test]
    fn finds_the_best_readme_regardless_of_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in ["README.txt", "Readme.md", "README.rst", "readme-notes.md"] {
            fs::write(dir.path().join(name), "# Hello\n").expect("write");
        }
        assert_eq!(
            read(dir.path()),
            Readme::Found {
                file_name: "Readme.md".into(),
                blocks: vec![heading(1, "Hello")],
                is_cut_short: false,
            }
        );
    }

    #[test]
    fn a_project_without_a_readme_file_has_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("main.c"), "int main;\n").expect("write");
        fs::create_dir(dir.path().join("README")).expect("a folder, not a file");
        assert_eq!(read(dir.path()), Readme::Missing);
        fs::write(dir.path().join("readme.txt"), "Plain.\n").expect("write");
        assert_eq!(
            read(dir.path()),
            Readme::Found {
                file_name: "readme.txt".into(),
                blocks: vec![text("Plain.")],
                is_cut_short: false,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_or_huge_readme_is_not_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink("/etc/hosts", dir.path().join("README.md")).expect("link");
        assert_eq!(
            read(dir.path()),
            Readme::Unreadable {
                file_name: "README.md".into(),
                error: "not a regular file (symlinks are not followed)".into(),
            }
        );

        let big = tempfile::tempdir().expect("tempdir");
        let text = "x".repeat(MAX_README_BYTES + 1);
        fs::write(big.path().join("README"), text).expect("write");
        let Readme::Unreadable { error, .. } = read(big.path()) else {
            panic!("a huge README was read");
        };
        assert!(error.contains("larger than"), "{error}");
    }
}
