use egui_kittest::kittest::Queryable;

use super::*;
use crate::analysis::fuzz::makefile::Makefile;
use crate::app::dialogs;

fn view_with(dictionary: Option<Dictionary>) -> HarnessView {
    let libfuzzer = Harness {
        file_name: "fuzz_parse.cc".into(),
        code: "// TODO: one\nint x;  // TODO: two\n".into(),
    };
    let afl = Harness {
        file_name: "fuzz_parse_afl.cc".into(),
        code: "// TODO: one\n__AFL_FUZZ_INIT();\n".into(),
    };
    let makefile = Makefile {
        file_name: "fuzz_parse.mk".into(),
        text: "# TODO: add to SOURCES: x\nROOT = .\nNAME := fuzz_parse\n".into(),
    };
    let drafts = Drafts {
        libfuzzer,
        afl,
        dictionary,
        makefile,
    };
    HarnessView::new(drafts, "parse")
}

fn view() -> HarnessView {
    let dictionary = Dictionary {
        file_name: "fuzz_parse.dict".into(),
        text: "# For afl-fuzz -x\n\"PING\"\n\":\"\n".into(),
        token_count: 2,
    };
    view_with(Some(dictionary))
}

fn window(view: HarnessView) -> egui_kittest::Harness<'static, HarnessView> {
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(900.0, 700.0))
        .build_ui_state(
            |ui, view: &mut HarnessView| {
                let ctx = ui.ctx().clone();
                view.show(&ctx, None);
            },
            view,
        );
    harness.run();
    harness
}

#[test]
fn shows_the_file_name_and_what_needs_a_look() {
    let harness = window(view());
    assert!(harness.query_by_label("fuzz_parse.cc").is_some());
    assert!(harness.query_by_label("2 TODOs to look at").is_some());
    assert!(harness.query_by_label("Copy").is_some());
    assert!(harness.query_by_label("Save As…").is_some());
}

#[test]
fn each_tab_shows_its_own_file() {
    let mut harness = window(view());
    harness.get_by_label("AFL++ harness").click();
    harness.run();
    assert!(harness.query_by_label("fuzz_parse_afl.cc").is_some());
    assert!(harness.query_by_label("1 TODO to look at").is_some());
    harness.get_by_label("Dictionary").click();
    harness.run();
    assert!(harness.query_by_label("fuzz_parse.dict").is_some());
    assert!(harness.query_by_label("2 tokens").is_some());
}

#[test]
fn without_constants_there_is_no_dictionary_to_show() {
    let mut harness = window(view_with(None));
    harness.get_by_label("Dictionary").click();
    harness.run();
    assert!(
        harness.query_by_label("fuzz_parse.cc").is_some(),
        "still the harness"
    );
}

#[test]
fn the_engine_picked_last_is_shown_first_next_time() {
    let mut harness = window(view());
    harness.get_by_label("AFL++ harness").click();
    harness.run();
    *harness.state_mut() = view();
    harness.run();
    assert!(harness.query_by_label("fuzz_parse_afl.cc").is_some());
}

#[test]
fn edits_are_kept_per_file_and_saved_with_its_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut view = view();
    view.libfuzzer.text.push_str("// edited\n");
    let path = dir.path().join("fuzz_parse.dict");
    view.dictionary.as_mut().expect("dictionary").save_to(&path);
    let written = std::fs::read_to_string(&path).expect("read");
    assert!(written.contains("\"PING\""));
    assert!(!written.contains("edited"));
    assert!(view.libfuzzer.text.ends_with("// edited\n"));
}

#[test]
fn saving_writes_the_edited_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fuzz_parse.cc");
    let mut view = view();
    view.libfuzzer.text.push_str("// edited\n");
    view.libfuzzer.save_to(&path);
    assert_eq!(view.libfuzzer.saved, Some(Saved::To(path.clone())));
    let written = std::fs::read_to_string(&path).expect("read");
    assert!(written.ends_with("// edited\n"));
}

#[test]
fn a_path_picked_in_the_save_dialog_gets_the_shown_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fuzz_parse_afl.cc");
    let picked = path.clone();
    let mut view = view();
    view.tab = Tab::Harness(Engine::Afl);
    let dialog = dialogs::spawn(&egui::Context::default(), move || Some(picked)).expect("spawn");
    view.save_dialog = Some((Tab::Harness(Engine::Afl), dialog));
    let mut harness = window(view);
    for _ in 0..400 {
        harness.step();
        if harness.state().save_dialog.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(harness.state().afl.saved, Some(Saved::To(path.clone())));
    let written = std::fs::read_to_string(&path).expect("read");
    assert!(written.contains("__AFL_FUZZ_INIT"));
}

#[cfg(unix)]
#[test]
fn saving_never_writes_through_a_symlink() {
    let dir = tempfile::tempdir().expect("tempdir");
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, "keep").expect("write");
    let link = dir.path().join("fuzz_parse.cc");
    std::os::unix::fs::symlink(&victim, &link).expect("symlink");
    let mut view = view();
    view.libfuzzer.save_to(&link);
    assert!(matches!(view.libfuzzer.saved, Some(Saved::Failed(_))));
    assert_eq!(std::fs::read_to_string(&victim).expect("read"), "keep");
}

#[test]
fn the_makefile_has_its_own_tab() {
    let mut harness = window(view());
    harness.get_by_label("Makefile").click();
    harness.run();
    assert!(harness.query_by_label("fuzz_parse.mk").is_some());
    assert!(harness.query_by_label("1 TODO to look at").is_some());
}

#[test]
fn a_saved_makefile_finds_the_project_from_where_it_is() {
    let project = tempfile::tempdir().expect("tempdir");
    let folder = project.path().join("fuzz");
    std::fs::create_dir(&folder).expect("mkdir");
    let mut view = view();
    view.root = Some(project.path().to_path_buf());
    view.save_draft(Tab::Makefile, &folder.join("fuzz_parse.mk"));
    let written = std::fs::read_to_string(folder.join("fuzz_parse.mk")).expect("read");
    assert!(written.contains("ROOT = ..\n"), "{written}");
    assert!(view.makefile.text.contains("ROOT = ..\n"), "shown as saved");
}

#[test]
fn save_all_writes_every_file_but_keeps_ones_already_there() {
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::write(project.path().join("fuzz_parse.cc"), "mine").expect("write");
    let mut view = view();
    view.root = Some(project.path().to_path_buf());
    view.save_all_to(project.path());
    let read = |name: &str| std::fs::read_to_string(project.path().join(name)).ok();
    assert_eq!(read("fuzz_parse.cc").as_deref(), Some("mine"), "kept");
    assert!(read("fuzz_parse_afl.cc").is_some_and(|t| t.contains("__AFL_FUZZ_INIT")));
    assert!(read("fuzz_parse.dict").is_some_and(|t| t.contains("PING")));
    assert!(read("fuzz_parse.mk").is_some_and(|t| t.contains("ROOT = .\n")));
    let summary = view.saved_all.clone().expect("a summary");
    assert!(summary.contains("3 files"), "{summary}");
    assert!(summary.contains("kept fuzz_parse.cc"), "{summary}");
}

#[test]
fn a_root_make_cannot_take_is_said_so() {
    let project = tempfile::tempdir().expect("tempdir");
    let spaced = project.path().join("my project");
    std::fs::create_dir(&spaced).expect("mkdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let mut view = view();
    view.root = Some(spaced);
    view.save_draft(Tab::Makefile, &elsewhere.path().join("fuzz_parse.mk"));
    assert!(
        matches!(&view.makefile.saved, Some(Saved::Note(_, note)) if note.contains("Set ROOT")),
        "{:?}",
        view.makefile.saved
    );
}
