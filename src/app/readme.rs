//! The project's README, for a quick read: the one at the top of the
//! project, read without following symlinks and only up to a size, then
//! read as Markdown (see `markdown`) or split into paragraphs. It comes
//! from the audited tree, so it is only ever shown as text, its hidden
//! characters marked and its size bounded: nothing in it is followed,
//! fetched or run.

use std::fs;
use std::path::Path;

pub use super::markdown::{Block, Run};

use super::markdown;
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
/// A table shows this many rows and columns at most.
const MAX_TABLE_ROWS: usize = 500;
const MAX_TABLE_COLUMNS: usize = 20;
/// Cells shown in all the README's tables together.
const MAX_TABLE_CELLS: usize = 5_000;

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
    // Every cell is drawn on its own, so all tables together get this many.
    let mut cells_left = MAX_TABLE_CELLS;
    let shown = blocks
        .into_iter()
        .take(MAX_BLOCKS)
        .map(|block| match block {
            Block::Heading { level, runs } => Block::Heading {
                level,
                runs: capped(runs),
            },
            Block::Paragraph(runs) => Block::Paragraph(capped(runs)),
            Block::Quote(runs) => Block::Quote(capped(runs)),
            Block::Item {
                depth,
                marker,
                runs,
            } => Block::Item {
                depth,
                marker: visible(&marker).into_owned(),
                runs: capped(runs),
            },
            Block::Code(code) => Block::Code(visible_lines(&cut(&code, MAX_BLOCK_CHARS))),
            Block::Rule => Block::Rule,
            Block::Table(rows) => {
                let mut budget = MAX_BLOCK_CHARS;
                let mut shown_rows = Vec::new();
                for row in rows.into_iter().take(MAX_TABLE_ROWS) {
                    let width = row.len().min(MAX_TABLE_COLUMNS);
                    if width > cells_left {
                        break;
                    }
                    cells_left -= width;
                    let cells = row.into_iter().take(width);
                    shown_rows.push(cells.map(|cell| shown_runs(cell, &mut budget)).collect());
                }
                Block::Table(shown_rows)
            }
        })
        .collect();
    (shown, is_cut_short)
}

/// `runs` cut to one block's worth of characters, hidden ones marked.
fn capped(runs: Vec<Run>) -> Vec<Run> {
    let mut budget = MAX_BLOCK_CHARS;
    shown_runs(runs, &mut budget)
}

/// `runs` cut to the characters left in `budget` (which they use up),
/// their hidden characters marked, links included.
fn shown_runs(runs: Vec<Run>, budget: &mut usize) -> Vec<Run> {
    let mut shown = Vec::new();
    for run in runs {
        if *budget == 0 {
            break;
        }
        let text = cut(&run.text, *budget);
        *budget = budget.saturating_sub(run.text.chars().count());
        shown.push(Run {
            text: visible_lines(&text),
            link: run
                .link
                .map(|link| visible(&cut(&link, MAX_LINK_CHARS)).into_owned()),
            ..run
        });
    }
    shown
}

/// A link's address is shown cut to this many characters.
const MAX_LINK_CHARS: usize = 300;

/// `text`, read as Markdown, ready to show: hidden characters marked and
/// cut to size, as a README is.
pub fn shown_markdown(text: &str) -> Vec<Block> {
    ready_to_show(markdown::blocks(text)).0
}

/// `text` as blocks: Markdown when `is_markdown`, else paragraphs as
/// written.
pub fn blocks(text: &str, is_markdown: bool) -> Vec<Block> {
    match is_markdown {
        true => markdown::blocks(text),
        false => paragraphs(&text.lines().collect::<Vec<_>>()),
    }
}

/// `lines` as paragraphs: runs of lines between blank ones.
fn paragraphs(lines: &[&str]) -> Vec<Block> {
    lines
        .split(|line| line.trim().is_empty())
        .filter(|run| !run.is_empty())
        .map(|run| {
            Block::Paragraph(vec![Run {
                text: run.join("\n"),
                ..Run::default()
            }])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &str) -> Run {
        Run {
            text: s.into(),
            ..Run::default()
        }
    }

    fn text(s: &str) -> Block {
        Block::Paragraph(vec![run(s)])
    }

    fn heading(level: u8, s: &str) -> Block {
        Block::Heading {
            level,
            runs: vec![run(s)],
        }
    }

    #[test]
    fn markdown_is_read_as_markdown() {
        let readme = "# zlib\n\nA compression library.\n\n## Build ##\n```sh\nmake\n```\n";
        assert_eq!(
            blocks(readme, true),
            [
                heading(1, "zlib"),
                text("A compression library."),
                heading(2, "Build"),
                Block::Code("make".into()),
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
        let many_headings = "# h\n".repeat(MAX_BLOCKS + 10);
        let (shown, is_cut_short) = ready_to_show(blocks(&many_headings, true));
        assert_eq!((shown.len(), is_cut_short), (MAX_BLOCKS, true));

        let one_long_line = "x".repeat(MAX_BLOCK_CHARS * 3);
        let (shown, is_cut_short) = ready_to_show(blocks(&one_long_line, true));
        let [Block::Paragraph(runs)] = shown.as_slice() else {
            panic!("{shown:?}");
        };
        let text = &runs[0].text;
        assert_eq!(text.chars().count(), MAX_BLOCK_CHARS + 1);
        assert!(text.ends_with('…') && !is_cut_short);
    }

    #[test]
    fn many_tables_share_one_budget_of_cells() {
        let table = "| a | b |\n|---|---|\n| 1 | 2 |\n\n";
        let (shown, _) = ready_to_show(blocks(&table.repeat(MAX_TABLE_CELLS), true));
        let cells: usize = shown
            .iter()
            .map(|b| match b {
                Block::Table(rows) => rows.iter().map(Vec::len).sum(),
                _ => 0,
            })
            .sum();
        assert!(cells <= MAX_TABLE_CELLS, "{cells}");
    }

    #[test]
    fn hidden_characters_are_marked_once_when_read() {
        let readme = "# a\u{202e}b\n\nc\u{200b}d [x](http://e\u{202e}vil)\n";
        let (shown, _) = ready_to_show(blocks(readme, true));
        let [Block::Heading { runs: heading, .. }, Block::Paragraph(body)] = shown.as_slice()
        else {
            panic!("{shown:?}");
        };
        assert!(!heading[0].text.contains('\u{202e}') && !body[0].text.contains('\u{200b}'));
        assert!(!body[1].link.as_deref().unwrap_or("").contains('\u{202e}'));
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
