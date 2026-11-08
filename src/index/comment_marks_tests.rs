use super::*;
use crate::index::graph_of_files;

fn marks_in(source: &str, limit: usize) -> FileMarks {
    marks_within(
        source,
        MarkLimits {
            tags: limit,
            silencers: limit,
        },
    )
}

fn marks_within(source: &str, limits: MarkLimits) -> FileMarks {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .expect("C++ grammar");
    let tree = parser.parse(source, None).expect("parse");
    collect(tree.root_node(), source, limits)
}

fn kinds(source: &str) -> Vec<CommentMarkKind> {
    marks_in(source, MAX_MARKS_PER_FILE)
        .marks
        .into_iter()
        .map(|mark| mark.kind)
        .collect()
}

fn silences(analyzer: Analyzer) -> CommentMarkKind {
    CommentMarkKind::Silences(analyzer)
}

fn tag(tag: SourceTag) -> CommentMarkKind {
    CommentMarkKind::Tag(tag)
}

#[test]
fn finds_each_analyzers_directive() {
    let cases = [
        ("// nosemgrep", Analyzer::Semgrep),
        ("// NOSEM: c.lang.gets", Analyzer::Semgrep),
        ("/* Flawfinder: ignore */", Analyzer::Flawfinder),
        ("// flawfinder:ignore", Analyzer::Flawfinder),
        ("// RATS : ignore", Analyzer::Flawfinder),
        ("// cppcheck-suppress nullPointer", Analyzer::Cppcheck),
        ("// cppcheck-suppress-file uninitvar", Analyzer::Cppcheck),
        ("// NOLINTNEXTLINE(bugprone-*)", Analyzer::ClangTidy),
        ("// coverity[tainted_data]", Analyzer::Coverity),
        ("// lgtm[cpp/unbounded-write]", Analyzer::CodeQl),
        ("// codeql[cpp/overflow]", Analyzer::CodeQl),
        ("// NOSONAR", Analyzer::SonarQube),
        ("//-V547", Analyzer::PvsStudio),
        ("/*@ignore@*/", Analyzer::Splint),
    ];
    for (comment, analyzer) in cases {
        let source = format!("int x; {comment}\n");
        assert_eq!(kinds(&source), [silences(analyzer)], "{comment}");
    }
}

#[test]
fn prose_that_only_looks_like_a_directive_is_not_one() {
    for comment in [
        "// nosemgrepx and xnosem",
        "// flawfinder ignores this",
        "// nolint, in lower case",
        "// rats: we should ignore? no: ratsignore",
        "//-Value",
    ] {
        let source = format!("int x; {comment}\n");
        assert!(kinds(&source).is_empty(), "{comment}");
    }
}

#[test]
fn tags_are_capitals_as_whole_words() {
    let found = kinds(
        "// TODO: check len\n\
         // FIXME(joe) leak\n\
         /* XXX */\n\
         // HACK - see #12\n\
         // BUG: off by one\n\
         /// \\todo Doxygen\n\
         /** @bug Doxygen too */\n",
    );
    assert_eq!(
        found,
        [
            tag(SourceTag::Todo),
            tag(SourceTag::Fixme),
            tag(SourceTag::Xxx),
            tag(SourceTag::Hack),
            tag(SourceTag::Bug),
            tag(SourceTag::Todo),
            tag(SourceTag::Bug),
        ]
    );
    for prose in [
        "// a todo list",
        "// TODOs left",
        "// BUG_ON(x)",
        "// XXXX",
        "// debugging",
        "// \\todolist",
    ] {
        assert!(kinds(&format!("{prose}\n")).is_empty(), "{prose}");
    }
}

#[test]
fn only_comments_are_read() {
    let source = "const char *s = \"TODO nosemgrep\";\nint TODO = 1;\n#define FIXME 2\n";
    assert!(kinds(source).is_empty());
}

#[test]
fn says_where_each_mark_is_and_what_it_says() {
    let source = "int a;\n  int b; // TODO: check b */\n/* first\n   XXX second line */\n";
    let marks = marks_in(source, MAX_MARKS_PER_FILE).marks;
    let places: Vec<(u32, u32, u32, &str)> = marks
        .iter()
        .map(|m| (m.span.line, m.span.start, m.span.end, m.text.as_str()))
        .collect();
    assert_eq!(
        places,
        [(2, 12, 16, "TODO: check b"), (4, 3, 6, "XXX second line"),]
    );
    assert_eq!(&source.lines().nth(1).unwrap()[12..16], "TODO");
}

#[test]
fn one_line_can_carry_several_marks_in_order() {
    assert_eq!(
        kinds("int x; // FIXME: unsafe, NOLINT for now\n"),
        [tag(SourceTag::Fixme), silences(Analyzer::ClangTidy)]
    );
}

#[test]
fn long_comments_are_cut_short() {
    let source = format!("// TODO {}\n", "x".repeat(MAX_MARK_CHARS * 2));
    let marks = marks_in(&source, MAX_MARKS_PER_FILE).marks;
    assert_eq!(marks[0].text.chars().count(), MAX_MARK_CHARS + 1);
    assert!(marks[0].text.ends_with('…'));
}

#[test]
fn a_file_of_too_many_marks_is_cut_short_and_says_so() {
    let source = "// TODO\n".repeat(5);
    let kept = marks_in(&source, 3);
    assert_eq!(kept.marks.len(), 3);
    assert!(kept.are_tags_cut_short && !kept.are_silencers_cut_short);
    let exact = marks_in(&source, 5);
    assert_eq!(exact.marks.len(), 5);
    assert!(!exact.are_tags_cut_short);
}

#[test]
fn a_flood_of_tags_cannot_crowd_out_silencing_comments() {
    let source = format!("{}int x; // nosemgrep\n", "// TODO\n".repeat(10));
    let kept = marks_within(
        &source,
        MarkLimits {
            tags: 3,
            silencers: 3,
        },
    );
    assert_eq!(kept.counts(), (3, 1));
    assert_eq!(
        kept.marks.last().map(|m| m.kind),
        Some(silences(Analyzer::Semgrep))
    );
    assert!(kept.are_tags_cut_short && !kept.are_silencers_cut_short);
}

#[test]
fn hostile_text_on_a_marked_line_is_kept_as_written() {
    let source = "// TODO \u{202E}evil\n";
    let marks = marks_in(source, MAX_MARKS_PER_FILE).marks;
    assert_eq!(marks[0].text, "TODO \u{202E}evil");
}

#[test]
fn marks_are_placed_in_the_function_they_are_in() {
    let graph = graph_of_files(&[(
        "a.c",
        "// TODO: file scope\n\
         int f(void) {\n\
           return 0; // nosemgrep\n\
         }\n",
    )]);
    let marks = &graph.comment_marks["a.c"];
    assert_eq!(marks.len(), 2);
    assert_eq!(marks[0].function, None);
    let f = graph.symbols.values().find(|s| s.name == "f").expect("f");
    assert_eq!(marks[1].function, Some(f.id));
    assert_eq!(marks[1].kind, silences(Analyzer::Semgrep));
    assert!(graph.tags_cut_short.is_empty() && graph.silencers_cut_short.is_empty());
}

#[test]
fn files_without_marks_have_no_entry() {
    let graph = graph_of_files(&[("a.c", "// nothing here\nint f(void) { return 0; }\n")]);
    assert!(graph.comment_marks.is_empty());
}
