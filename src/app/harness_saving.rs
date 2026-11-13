//! Saving what the fuzz harness window drafted: one file where you choose
//! (Save As…), or all of them into a folder (Save All…), never writing
//! through a symlink. A saved Makefile gets its ROOT set for where it is.

use std::path::{Path, PathBuf};

use super::super::dialogs::{self, DialogState};
use super::{HarnessView, Tab};
use crate::analysis::fuzz::harness::Engine;
use crate::analysis::fuzz::makefile::{root_for, with_root};
use crate::safe_fs;

/// Every tab, in the order Save All writes them.
const ALL_TABS: [Tab; 4] = [
    Tab::Harness(Engine::LibFuzzer),
    Tab::Harness(Engine::Afl),
    Tab::Dictionary,
    Tab::Makefile,
];

/// What the last save of a file did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Saved {
    To(PathBuf),
    /// Saved, with something to know about it.
    Note(PathBuf, String),
    Failed(String),
}

/// One drafted file, as edited.
pub(super) struct Draft {
    pub(super) file_name: String,
    pub(super) text: String,
    pub(super) saved: Option<Saved>,
}

impl Draft {
    pub(super) fn new(file_name: String, text: String) -> Self {
        Self {
            file_name,
            text,
            saved: None,
        }
    }

    /// Writes the text to `path`, replacing what's there but never
    /// writing through a symlink.
    pub(super) fn save_to(&mut self, path: &Path) {
        self.saved = Some(match write(path, &self.text) {
            Ok(()) => Saved::To(path.to_path_buf()),
            Err(error) => Saved::Failed(error),
        });
    }
}

impl HarnessView {
    /// Saves `tab`'s file to `path`; a Makefile with ROOT pointing from
    /// there to the project.
    pub(super) fn save_draft(&mut self, tab: Tab, path: &Path) {
        if tab == Tab::Makefile {
            return self.save_makefile(path);
        }
        if let Some(draft) = self.draft_mut(tab) {
            draft.save_to(path);
        }
    }

    /// Saves the Makefile with its ROOT set for `path`, and shows it so;
    /// or says to set it by hand if the project's path can't go into it.
    fn save_makefile(&mut self, path: &Path) {
        let Some(project) = self.root.clone() else {
            return self.makefile.save_to(path);
        };
        let root = path.parent().and_then(|folder| root_for(folder, &project));
        let text = match &root {
            Some(root) => with_root(&self.makefile.text, root),
            None => self.makefile.text.clone(),
        };
        self.makefile.saved = Some(match (write(path, &text), root) {
            (Err(error), _) => Saved::Failed(error),
            (Ok(()), Some(_)) => {
                self.makefile.text = text;
                Saved::To(path.to_path_buf())
            }
            (Ok(()), None) => Saved::Note(
                path.to_path_buf(),
                "Set ROOT in it to the project's folder: its path has characters make can't take."
                    .into(),
            ),
        });
    }

    /// Saves every file into `folder`, but keeps any already there.
    pub(super) fn save_all_to(&mut self, folder: &Path) {
        let (mut saved, mut kept, mut failed) = (0, Vec::new(), Vec::new());
        for tab in ALL_TABS {
            let Some(name) = self.draft_mut(tab).map(|d| d.file_name.clone()) else {
                continue;
            };
            let path = folder.join(&name);
            if path.symlink_metadata().is_ok() {
                kept.push(name);
                continue;
            }
            self.save_draft(tab, &path);
            match self.draft_mut(tab).and_then(|d| d.saved.clone()) {
                Some(Saved::To(_) | Saved::Note(..)) => saved += 1,
                Some(Saved::Failed(error)) => failed.push(format!("{name} ({error})")),
                None => {}
            }
        }
        self.saved_all = Some(save_all_summary(folder, saved, &kept, &failed));
    }

    /// Asks where to save `tab`'s file, off the UI code (see `dialogs`);
    /// `poll_dialogs` saves there.
    pub(super) fn save_as(&mut self, ctx: &egui::Context, tab: Tab) {
        self.saved_all = None;
        let Some(file_name) = self.draft_mut(tab).map(|d| d.file_name.clone()) else {
            return;
        };
        let root = self.root.clone();
        let dialog = dialogs::spawn(ctx, move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Save the fuzzing file")
                .set_file_name(file_name);
            match root {
                Some(root) => dialog.set_directory(root),
                None => dialog,
            }
            .save_file()
        });
        match dialog {
            Ok(dialog) => self.save_dialog = Some((tab, dialog)),
            Err(error) => {
                if let Some(draft) = self.draft_mut(tab) {
                    draft.saved = Some(Saved::Failed(error));
                }
            }
        }
    }

    /// Asks for a folder to save every file into.
    pub(super) fn save_all(&mut self, ctx: &egui::Context) {
        let root = self.root.clone();
        let dialog = dialogs::spawn(ctx, move || {
            let dialog = rfd::FileDialog::new().set_title("Save the fuzzing files into");
            match root {
                Some(root) => dialog.set_directory(root),
                None => dialog,
            }
            .pick_folder()
        });
        match dialog {
            Ok(dialog) => self.save_all_dialog = Some(dialog),
            Err(error) => self.saved_all = Some(format!("Could not save: {error}")),
        }
    }

    /// Saves where a dialog closed with a path picked.
    pub(super) fn poll_dialogs(&mut self) {
        if let Some((tab, state)) = self.save_dialog.as_ref().map(|(t, d)| (*t, d.poll())) {
            if state != DialogState::Open {
                self.save_dialog = None;
            }
            match state {
                DialogState::Open | DialogState::Cancelled => {}
                DialogState::Picked(path) => self.save_draft(tab, &path),
                DialogState::Failed(error) => {
                    if let Some(draft) = self.draft_mut(tab) {
                        draft.saved = Some(Saved::Failed(error));
                    }
                }
            }
        }
        let Some(state) = self.save_all_dialog.as_ref().map(|d| d.poll()) else {
            return;
        };
        if state != DialogState::Open {
            self.save_all_dialog = None;
        }
        match state {
            DialogState::Open | DialogState::Cancelled => {}
            DialogState::Picked(folder) => self.save_all_to(&folder),
            DialogState::Failed(error) => self.saved_all = Some(format!("Could not save: {error}")),
        }
    }
}

/// Writes `text` to `path`, replacing what's there but never writing
/// through a symlink.
fn write(path: &Path, text: &str) -> Result<(), String> {
    match path.parent().zip(path.file_name().and_then(|n| n.to_str())) {
        Some((dir, name)) => safe_fs::write_atomically(dir, name, text).map_err(|e| e.to_string()),
        None => Err("that isn't a file path".into()),
    }
}

/// What Save All did: `Saved 3 files to …; kept fuzz_x.cc, which was
/// already there (Save As… replaces it).`
fn save_all_summary(folder: &Path, saved: usize, kept: &[String], failed: &[String]) -> String {
    let files = if saved == 1 { "file" } else { "files" };
    let mut summary = format!("Saved {saved} {files} to {}", folder.display());
    if !kept.is_empty() {
        let (which, was) = match kept.len() {
            1 => ("it", "was"),
            _ => ("them", "were"),
        };
        summary.push_str(&format!(
            "; kept {}, which {was} already there (Save As… replaces {which})",
            kept.join(", ")
        ));
    }
    if !failed.is_empty() {
        summary.push_str(&format!("; could not save {}", failed.join(", ")));
    }
    summary.push('.');
    summary
}
