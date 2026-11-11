use super::*;

fn run(text: &str) -> Run {
    plain(text)
}

fn styled(text: &str, restyle: impl FnOnce(&mut Style)) -> Run {
    let mut style = Style::default();
    restyle(&mut style);
    Run {
        text: text.into(),
        style,
        link: None,
    }
}

#[test]
fn headings_paragraphs_and_code() {
    let readme = "# zlib\n\nA *small* and **fast** library.\nUse `inflate()`.\n\n```sh\nmake\n# not a heading\n```\n";
    assert_eq!(
        blocks(readme),
        [
            Block::Heading {
                level: 1,
                runs: vec![run("zlib")]
            },
            Block::Paragraph(vec![
                run("A "),
                styled("small", |s| s.is_emphasis = true),
                run(" and "),
                styled("fast", |s| s.is_strong = true),
                run(" library. Use "),
                styled("inflate()", |s| s.is_code = true),
                run("."),
            ]),
            Block::Code("make\n# not a heading".into()),
        ]
    );
}

#[test]
fn lists_nest_number_and_tick() {
    let readme = "- one\n  1. first\n  2. second\n- [x] done\n- [ ] not yet\n";
    let items: Vec<(u8, String, String)> = blocks(readme)
        .into_iter()
        .filter_map(|b| match b {
            Block::Item {
                depth,
                marker,
                runs,
            } => Some((
                depth,
                marker,
                runs.iter().map(|r| r.text.as_str()).collect(),
            )),
            _ => None,
        })
        .collect();
    let expected = [
        (0, "•", "one"),
        (1, "1.", "first"),
        (1, "2.", "second"),
        (0, "☑", "done"),
        (0, "☐", "not yet"),
    ];
    let expected: Vec<(u8, String, String)> = expected
        .iter()
        .map(|(d, m, t)| (*d, m.to_string(), t.to_string()))
        .collect();
    assert_eq!(items, expected);
}

#[test]
fn links_keep_their_address_and_images_their_description() {
    let readme = "See [the docs](https://example.com/docs) and ![a logo](logo.png).\n";
    let found = blocks(readme);
    let [Block::Paragraph(runs)] = found.as_slice() else {
        panic!("one paragraph");
    };
    assert_eq!(runs[1].text, "the docs");
    assert_eq!(runs[1].link.as_deref(), Some("https://example.com/docs"));
    assert_eq!(runs[3].text, "[image: a logo]");
    assert!(runs[3].style.is_image);
}

#[test]
fn html_becomes_its_text() {
    let readme = "<p align=\"center\"><img src=\"x.png\" alt=\"x\"/>\n<b>Fast</b> tool</p>\n\nText <span>here</span>.\n";
    let found = blocks(readme);
    assert_eq!(found[0], Block::Paragraph(vec![run("Fast tool")]));
    let Block::Paragraph(runs) = &found[1] else {
        panic!("{found:?}");
    };
    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(text, "Text here.");
}

#[test]
fn tables_quotes_and_rules() {
    let readme = "| a | b |\n|---|---|\n| 1 | 2 |\n\n> quoted\n\n---\n";
    let found = blocks(readme);
    assert_eq!(
        found[0],
        Block::Table(vec![
            vec![vec![run("a")], vec![run("b")]],
            vec![vec![run("1")], vec![run("2")]],
        ])
    );
    assert_eq!(found[1], Block::Quote(vec![run("quoted")]));
    assert_eq!(found[2], Block::Rule);
}

#[test]
fn deep_nesting_is_read_without_trouble() {
    let readme = format!("{}x\n", "> ".repeat(500)) + &"- a\n".repeat(3);
    let found = blocks(&readme);
    assert!(found.iter().any(|b| matches!(b, Block::Quote(_))));
}

#[test]
fn a_loose_list_has_one_row_per_item() {
    let readme = "- one\n\n- two\n\n  more of two\n\n- three\n";
    let rows: Vec<(String, String)> = blocks(readme)
        .into_iter()
        .filter_map(|b| match b {
            Block::Item { marker, runs, .. } => {
                Some((marker, runs.iter().map(|r| r.text.as_str()).collect()))
            }
            _ => None,
        })
        .collect();
    let expected = [
        ("•", "one"),
        ("•", "two"),
        ("", "more of two"),
        ("•", "three"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(m, t)| (m.to_string(), t.to_string()))
        .collect();
    assert_eq!(rows, expected);
}

#[test]
fn an_item_starting_with_code_shows_its_marker_first() {
    let found = blocks("1. ```\n   make\n   ```\n");
    assert!(
        matches!(&found[0], Block::Item { marker, .. } if marker == "1."),
        "{found:?}"
    );
    assert_eq!(found[1], Block::Code("make".into()));
}

#[test]
fn a_tag_over_lines_and_code_in_an_image_stay_out_of_the_text() {
    let found = blocks("<a\n href=\"x\">link</a>\n\n![a `b` c](i.png)\n");
    assert_eq!(found[0], Block::Paragraph(vec![run("link")]));
    let Block::Paragraph(runs) = &found[1] else {
        panic!("{found:?}");
    };
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].text, "[image: a b c]");
}

#[test]
fn a_crafted_readme_stays_small_while_it_is_read() {
    // Alternating styles inside one huge link: every run would copy it.
    let styles = "*a*b".repeat(100_000);
    let readme = format!("[{styles}](https://e.com/{})\n", "x".repeat(500_000));
    let found = blocks(&readme);
    let [Block::Paragraph(runs)] = found.as_slice() else {
        panic!("one paragraph");
    };
    assert!(runs.len() <= MAX_BLOCK_RUNS);
    let chars: usize = runs.iter().map(|r| r.text.chars().count()).sum();
    assert!(chars <= MAX_BLOCK_CHARS, "{chars}");
    assert!(runs
        .iter()
        .all(|r| r.link.as_ref().is_some_and(|l| l.len() <= MAX_LINK_CHARS)));
}
