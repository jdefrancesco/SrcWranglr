//! What the app does for agents: serving each project it opens, picking
//! up their calls for the Agent pane (following one if asked), making the
//! writes they ask for (notes, journal entries, SARIF imports), and the
//! Assessment tab's buttons (start an AAVR run in opencode, connect it).

use super::aavr_view::AavrAction;
use super::assessment;
use super::background::BackgroundJob;
use super::editor::{self, SourcePosition};

use super::agent_bridge::{AgentBridge, AgentWrite};
use super::agent_panel::AgentPanelAction;
use super::in_app_agent::{InAppRun, ToMain, START_PROMPT};
use super::{SrcWranglrApp, Status};
use crate::mcp::dispatch::Place;
use crate::mcp::tools::notes::NoteChange;
use crate::mcp::tools::verdicts::VerdictChange;

/// The task for a function-scoped review: judge each memory operation's safety.
fn function_review_prompt(function: &str, file: &str, line: u32) -> String {
    format!(
        "Focus only on the function `{function}` at {file}:{line}. For each memory operation in \
         it (memcpy, memmove, strcpy, strncpy, sprintf, realloc, and writes through pointers or \
         arrays), work out the destination buffer's size by tracing its allocation, the number \
         of bytes written, and whether any input can make the write exceed the buffer. Say \
         whether each is safe, and whether it is reachable from untrusted input. Use \
         function_source and the trace tools; read the code rather than guessing. End with a \
         per-operation and an overall verdict for the function."
    )
}

impl SrcWranglrApp {
    /// Serves the project just opened to agents, instead of the last one.
    pub(super) fn serve_agents(&mut self, ctx: &egui::Context) {
        // The old one lets go of its socket first: reopening the same
        // folder needs it.
        self.agent = AgentBridge::default();
        self.agent = AgentBridge::start(&self.project, ctx);
        self.agent_panel.reset();
    }

    /// Agents' calls and writes since the last frame.
    pub(super) fn poll_agent(&mut self, ctx: &egui::Context) {
        let (events, writes) = self.agent.drain();
        for request in writes {
            if request.is_stale() {
                continue;
            }
            let answer = self.make_agent_write(&request.write, ctx);
            // The agent may have given up waiting.
            let _ = request.reply.send(answer);
        }
        for event in events {
            if self.agent_panel.is_following() {
                if let Some(place) = &event.place {
                    self.show_place(place);
                }
            }
            self.agent_panel.push(event);
        }
    }

    fn make_agent_write(
        &mut self,
        write: &AgentWrite,
        ctx: &egui::Context,
    ) -> Result<String, String> {
        match write {
            AgentWrite::Note(change) => self.save_agent_note(change),
            AgentWrite::Journal(entry) => {
                let journal = self.journal.with_added(entry.clone())?;
                if !self.set_journal(journal) {
                    return Err("SrcWranglr couldn't save the journal; the user sees why.".into());
                }
                Ok(crate::mcp::tools::records::written(
                    self.journal.entries().len(),
                ))
            }
            AgentWrite::Sarif(path) => {
                if self.sarif_import.is_some() {
                    return Err(
                        "SrcWranglr is already importing a report; try again shortly.".into(),
                    );
                }
                self.read_sarif(path.clone(), ctx);
                Ok("SrcWranglr is reading the report; its results will be in the user's Findings, under each tool's name. (Your findings tool lists SrcWranglr's own findings only.)".into())
            }
            // A dismissing verdict, clearing one, or any on a prompt
            // injection waits for the user; an escalating one applies now.
            AgentWrite::Verdict(change) if change.needs_approval() => {
                if self.agent_panel.queue_approval(change.clone()) {
                    Ok(format!(
                        "Asked the user to approve ({}). It isn't applied until they do; check a later findings call for the result.",
                        change.summary()
                    ))
                } else {
                    Err("SrcWranglr's approval queue is full; ask the user to clear some in the Agent pane, then try again.".into())
                }
            }
            AgentWrite::Verdict(change) => self.apply_agent_verdict(change),
        }
    }

    /// Applies an agent's verdict to the matching finding, attributed to
    /// the agent, and says how it went.
    /// Applies several approved agent verdicts; a one-line summary.
    fn apply_agent_verdicts(&mut self, changes: &[VerdictChange]) -> String {
        let (mut applied, mut failed) = (0, 0);
        for change in changes {
            match self.apply_agent_verdict(change) {
                Ok(_) => applied += 1,
                Err(_) => failed += 1,
            }
        }
        let failed = match failed {
            0 => String::new(),
            n => format!(" ({n} could not be applied)"),
        };
        format!(
            "Applied {}{failed}.",
            crate::mcp::format::count(applied, "agent verdict")
        )
    }

    fn apply_agent_verdict(&mut self, change: &VerdictChange) -> Result<String, String> {
        let finding = self
            .findings
            .all()
            .iter()
            .find(|f| f.file == change.file && f.line == change.line && f.rule == change.rule)
            .cloned();
        let Some(finding) = finding else {
            return Err("That finding is no longer in the list; re-read findings.".into());
        };
        let note = match change.note.trim() {
            "" => "[agent]".to_string(),
            said => format!("[agent] {said}"),
        };
        self.apply_verdict(&finding, change.status, note);
        if let Status::Error(error) = &self.status {
            return Err(error.clone());
        }
        Ok(format!(
            "Recorded the verdict on {} at {}:{}; the user sees it in Findings.",
            change.rule, change.file, change.line
        ))
    }

    fn save_agent_note(&mut self, change: &NoteChange) -> Result<String, String> {
        let existing = self.notebook.notes().get(&change.target);
        let (text, is_starred) = change.applied_to(existing)?;
        self.notebook
            .set(&self.project, change.target.clone(), &text, is_starred)?;
        Ok(change.saved())
    }

    pub(super) fn show_agent_panel(&mut self, ui: &mut egui::Ui) {
        if self.agent_panel.needs_launcher() {
            let launcher = assessment::launcher(&self.settings.search_path());
            self.agent_panel.set_launcher(launcher);
        }
        let command = mcp_command();
        let root = self.project.root.clone();
        let serving = self.agent.serving().clone();
        let action = self
            .agent_panel
            .show(ui, self.layout, &serving, (&command, root.as_deref()));
        match action {
            Some(AgentPanelAction::GoTo(place)) => self.show_place(&place),
            Some(AgentPanelAction::Assessment(action)) => self.on_assessment(action, ui.ctx()),
            Some(AgentPanelAction::ApplyVerdicts(changes)) => {
                self.status = Status::Info(self.apply_agent_verdicts(&changes));
            }
            Some(AgentPanelAction::RejectVerdicts(count)) => {
                self.status = Status::Info(format!(
                    "Rejected {}.",
                    crate::mcp::format::count(count, "agent verdict")
                ));
            }
            Some(AgentPanelAction::StopInApp) => self.stop_in_app_agent(),
            None => {}
        }
    }

    /// Brings the agent's confirmed AAVR candidates into the Findings
    /// window, rebuilt only when they change (the run is re-read on a
    /// timer). Rejected and speculative ones stay in the Assessment tab.
    pub(super) fn sync_agent_findings(&mut self) {
        let key = self
            .agent_panel
            .assessment_run()
            .map(super::agent_findings::key);
        if key == self.agent_findings_key {
            return;
        }
        self.agent_findings_key = key;
        let found = self
            .agent_panel
            .assessment_run()
            .map(|run| super::agent_findings::findings(run, &self.project.graph))
            .unwrap_or_default();
        self.findings = self
            .findings
            .with_tool_results(super::agent_findings::TOOL, found);
        self.refresh_verdicts();
    }

    /// Drains the in-app run's steps and requests each frame, and ends it
    /// when it's done.
    pub(super) fn poll_in_app(&mut self, ctx: &egui::Context) {
        let Some((events, requests, result)) = self.in_app_run.as_ref().map(|run| {
            let (events, requests) = run.drain();
            (events, requests, run.result())
        }) else {
            return;
        };
        for event in events {
            self.agent_panel.push_in_app(event);
        }
        for request in requests {
            match request {
                ToMain::Write { write, reply } => {
                    let _ = reply.send(self.make_agent_write(&write, ctx));
                }
                ToMain::Command { command, reply } => {
                    self.agent_panel.queue_command(command, reply);
                }
            }
        }
        if let Some(result) = result {
            self.agent_panel.finish_in_app(&result);
            self.in_app_run = None;
        }
    }

    /// Starts the in-app agent on the open project, or says what's missing.
    pub(super) fn start_in_app_agent(&mut self, ctx: &egui::Context) {
        self.begin_in_app_agent(START_PROMPT.to_string(), ctx);
    }

    /// Starts an in-app run scoped to one function, asking the agent to judge
    /// whether its memory operations are safe.
    pub(super) fn start_in_app_agent_for(
        &mut self,
        function: &str,
        file: &str,
        line: u32,
        ctx: &egui::Context,
    ) {
        self.begin_in_app_agent(function_review_prompt(function, file, line), ctx);
    }

    /// Shared setup for an in-app run with `user_prompt` as its task.
    fn begin_in_app_agent(&mut self, user_prompt: String, ctx: &egui::Context) {
        if !self.is_project_open || self.project.root.is_none() {
            self.status = Status::Info("Open a project folder first.".into());
            return;
        }
        let Some(provider) = self.settings.agent.provider() else {
            self.status = Status::Info(
                "Set the agent's model and API key in Settings (the [agent] section) first.".into(),
            );
            return;
        };
        let client = match crate::agent::openai::OpenAiClient::new(provider) {
            Ok(client) => Box::new(client) as Box<dyn crate::agent::llm::LlmClient>,
            Err(error) => {
                self.status = Status::Error(error);
                return;
            }
        };
        self.stop_in_app_agent();
        self.in_app_run = Some(InAppRun::start(
            client,
            self.project.name.clone(),
            self.project.root.clone(),
            std::sync::Arc::clone(&self.project.graph),
            user_prompt,
            ctx.clone(),
        ));
        self.agent_panel.set_in_app_running(true);
        self.agent_panel.set_open(true);
        self.status =
            Status::Info("The agent is running in SrcWranglr. Watch the Agent pane.".into());
    }

    /// Stops an in-app run and unblocks any thread waiting on approval.
    pub(super) fn stop_in_app_agent(&mut self) {
        if let Some(run) = self.in_app_run.take() {
            run.stop();
        }
        // Deny pending commands so a waiting thread winds down.
        self.agent_panel.finish_in_app(&Ok(String::new()));
    }

    fn on_assessment(&mut self, action: AavrAction, ctx: &egui::Context) {
        let search = self.settings.search_path();
        let root = self.project.root.clone();
        match action {
            AavrAction::RunInApp => self.start_in_app_agent(ctx),
            AavrAction::GoTo { file, line } => self.go_to_place(None, &file, line),
            AavrAction::FindSymbol(name) => match self.project.find(&name) {
                Some(id) => self.graph_view.select(id),
                None => self.status = Status::Info(format!("No symbol {name} in the index.")),
            },
            AavrAction::Start => {
                let launcher = assessment::launcher(&search);
                match (launcher.opencode, root) {
                    // An untrusted repo with its own opencode config, plugins
                    // or AGENTS.md can run code the moment opencode starts, so
                    // ask first.
                    (Some(opencode), Some(root))
                        if !super::trust::is_trusted(&root)
                            && assessment::would_run_repo_code(&root) =>
                    {
                        self.start_to_confirm = Some((opencode, root));
                    }
                    (Some(opencode), Some(root)) => {
                        self.start_assessment(&opencode, &root, &search);
                    }
                    _ => {
                        self.agent_panel.assessment.message =
                            Some(Err("Starting needs opencode and a project folder.".into()));
                    }
                }
            }
            AavrAction::Connect => {
                let Some(opencode) = search.find("opencode") else {
                    return;
                };
                let launch = assessment::connect_launch(&opencode, &assessment::this_exe());
                let job = BackgroundJob::spawn(
                    "Connecting SrcWranglr to opencode",
                    move |_| assessment::connect(&launch, &search),
                    ctx.clone(),
                );
                self.agent_panel.assessment.connect(job);
            }
            AavrAction::OpenReport(relative) => {
                let Some(root) = root else { return };
                // The report is written by the agent in a hostile tree; open
                // it only if it's still a real file in a real reports folder.
                let reports = root.join(super::aavr_run::AAVR_DIR).join("reports");
                let file = reports.join("aavr-report.md");
                let is_real = crate::safe_fs::is_real_dir(&reports).unwrap_or(false)
                    && std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file());
                if !is_real {
                    self.status =
                        Status::Error("The report is no longer a plain file there.".into());
                    return;
                }
                let position = SourcePosition {
                    path: std::path::absolute(&file).unwrap_or(file),
                    line: 1,
                    column: 1,
                };
                let setting = self.settings.editor.as_deref();
                self.status = match editor::open_in_editor(&position, setting, &search) {
                    Ok(program) => Status::Info(format!("Opened {relative} with {program}")),
                    Err(error) => Status::Error(error),
                };
            }
        }
    }

    /// Launches opencode on `root` for an assessment, reporting how it went.
    fn start_assessment(
        &mut self,
        opencode: &std::path::Path,
        root: &std::path::Path,
        search: &crate::programs::SearchPath,
    ) {
        self.agent_panel.assessment.message = Some(assessment::start(opencode, root, search));
    }

    /// The modal that confirms starting opencode on an untrusted repo.
    pub(super) fn show_start_confirm(&mut self, ctx: &egui::Context) {
        let Some((_, root)) = &self.start_to_confirm else {
            return;
        };
        let folder = root.display().to_string();
        let response = egui::Modal::new(egui::Id::unique("aavr_start_confirm")).show(ctx, |ui| {
            ui.heading("Start the agent on an untrusted project?");
            ui.label(
                "This project isn't trusted and has its own opencode config, plugins or AGENTS.md.                  opencode may run that code, and the repo could steer the agent. Start anyway?",
            );
            ui.monospace(&folder);
            ui.horizontal(|ui| {
                let go = ui.button("Start opencode").clicked();
                let cancel = ui.button("Cancel").clicked();
                (go, cancel)
            })
            .inner
        });
        let (go, cancel) = response.inner;
        if go {
            if let Some((opencode, root)) = self.start_to_confirm.take() {
                let search = self.settings.search_path();
                self.start_assessment(&opencode, &root, &search);
            }
        } else if cancel || response.should_close() {
            self.start_to_confirm = None;
        }
    }

    fn show_place(&mut self, place: &Place) {
        self.go_to_place(place.symbol, &place.file, place.line);
    }
}

/// What an agent's client runs to reach the app.
fn mcp_command() -> String {
    let exe = std::env::current_exe()
        .map(|exe| exe.display().to_string())
        .unwrap_or_else(|_| "srcwranglr".into());
    let exe = match exe.contains(' ') {
        true => format!("'{exe}'"),
        false => exe,
    };
    format!("{exe} {}", crate::mcp::MCP_ARG)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_function_review_prompt_names_the_function_and_place() {
        let prompt = function_review_prompt("hpack_append_hdr", "src/hpack.c", 406);
        assert!(prompt.contains("hpack_append_hdr"), "{prompt}");
        assert!(prompt.contains("src/hpack.c:406"), "{prompt}");
        assert!(prompt.contains("memcpy"), "{prompt}");
    }

    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::index::graph_of_files;
    use crate::notes::{store, NoteTarget};

    use crate::mcp::args::{VerdictArgs, VerdictStatus};
    use crate::mcp::tools::verdicts::verdict_change;
    use crate::mcp::workspace::Workspace;
    use crate::notes::triage::TriageStatus;
    use std::sync::Arc;

    /// An app on `dir` whose findings include a strcpy at m.c:2.
    fn app_with_a_finding(dir: &std::path::Path) -> SrcWranglrApp {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let source = "void f(char *s) {\n    strcpy(dst, s);\n}\n";
        app.project = Project::new(
            "p",
            Some(dir.to_path_buf()),
            graph_of_files(&[("m.c", source)]),
        );
        app.findings =
            crate::app::findings::Findings::for_graph(&app.project.graph, &Default::default());
        app
    }

    fn verdict(dir: &std::path::Path, status: VerdictStatus) -> VerdictChange {
        let graph = {
            let source = "void f(char *s) {\n    strcpy(dst, s);\n}\n";
            graph_of_files(&[("m.c", source)])
        };
        let ws = Workspace::from_shared("p", Some(dir.to_path_buf()), Arc::new(graph));
        verdict_change(
            &ws,
            &VerdictArgs {
                file: "m.c".into(),
                line: 2,
                rule: "strcpy".into(),
                status,
                note: Some("bounded".into()),
            },
        )
        .expect("a strcpy finding")
    }

    #[test]
    fn an_agents_confirm_applies_and_a_dismissal_waits_for_the_user() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = egui::Context::default();
        let mut app = app_with_a_finding(dir.path());
        // Trust it so verdicts apply without the trust gate.
        crate::app::trust::trust(dir.path()).expect("trust");
        app.are_project_files_trusted = true;

        let confirm = AgentWrite::Verdict(verdict(dir.path(), VerdictStatus::Confirmed));
        let said = app.make_agent_write(&confirm, &ctx).expect("applied");
        assert!(said.contains("Recorded the verdict"), "{said}");
        let triage = crate::notes::triage::load(dir.path()).expect("load");
        assert_eq!(triage.entries().len(), 1);
        assert_eq!(triage.entries()[0].status, TriageStatus::ConfirmedBug);
        assert!(triage.entries()[0].note.starts_with("[agent]"));

        // A false positive is queued, not applied.
        let dismiss = AgentWrite::Verdict(verdict(dir.path(), VerdictStatus::FalsePositive));
        let said = app.make_agent_write(&dismiss, &ctx).expect("queued");
        assert!(said.contains("Asked the user to approve"), "{said}");
        assert_eq!(
            crate::notes::triage::load(dir.path())
                .expect("load")
                .entries()[0]
                .status,
            TriageStatus::ConfirmedBug,
            "the dismissal isn't applied yet"
        );
    }

    #[test]
    fn an_agents_note_is_added_below_yours_and_saved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = graph_of_files(&[("x.c", "int f(void) { return 0; }\n")]);
        app.project = Project::new("p", Some(dir.path().to_path_buf()), graph);
        app.notebook = super::super::notebook::Notebook::open(&app.project);
        let target = NoteTarget::Line {
            file: "x.c".into(),
            line: 1,
        };
        app.notebook
            .set(&app.project, target.clone(), "mine", false)
            .expect("mine");
        let change = NoteChange {
            target: target.clone(),
            anchor: None,
            added: "[agent] returns 0".into(),
        };
        let said = app.save_agent_note(&change).expect("saved");
        assert!(said.contains("x.c:1"), "{said}");
        let saved = store::load(dir.path()).expect("load");
        assert_eq!(
            saved.get(&target).map(|n| n.text.as_str()),
            Some("mine\n\n[agent] returns 0")
        );
    }

    #[test]
    fn the_command_runs_this_binary() {
        let command = mcp_command();
        assert!(command.ends_with(" --mcp"), "{command}");
    }
}
