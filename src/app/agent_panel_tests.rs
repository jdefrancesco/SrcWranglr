use std::time::{Duration, Instant};

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;
use crate::mcp::dispatch::Place;
use crate::mcp::tools::verdicts::VerdictChange;
use crate::notes::triage::TriageStatus;

struct State {
    panel: AgentPanel,
    serving: Serving,
    action: Option<AgentPanelAction>,
}

fn event(tool: &str, place: Option<Place>, is_error: bool) -> AgentEvent {
    AgentEvent {
        at: Instant::now() - Duration::from_secs(90),
        tool: tool.into(),
        asked: "name=helper".into(),
        answer: if is_error {
            "No symbol `helper`.".into()
        } else {
            "Callers of helper  x.c:1".into()
        },
        is_error,
        place,
    }
}

fn harness(panel: AgentPanel) -> Harness<'static, State> {
    let mut panel = panel;
    panel.set_open(true);
    let mut harness = crate::app::fonts::test_harness()
        .with_size(egui::vec2(900.0, 600.0))
        .build_ui_state(
            |ui, state: &mut State| {
                let command = "srcwranglr --mcp";
                if let Some(action) =
                    state
                        .panel
                        .show(ui, Layout::default(), &state.serving, (command, None))
                {
                    state.action = Some(action);
                }
            },
            State {
                panel,
                serving: Serving::On {
                    folder: "/p".into(),
                },
                action: None,
            },
        );
    harness.run();
    harness
}

#[test]
fn before_any_call_it_says_how_to_connect_an_agent() {
    let harness = harness(AgentPanel::default());
    harness.get_by_label("Agent (0 calls)");
    harness.get_by_label_contains("No agent has asked anything yet");
    harness.get_by_label("srcwranglr --mcp");
    harness.get_by_label("opencode mcp add --global srcwranglr -- srcwranglr --mcp");
}

#[test]
fn calls_are_listed_newest_first_and_a_click_goes_where_it_looked() {
    let mut panel = AgentPanel::default();
    let place = Place {
        symbol: Some(3),
        file: "x.c".into(),
        line: 1,
    };
    panel.push(event("callers", Some(place.clone()), false));
    panel.push(event("function_source", None, true));
    let mut harness = harness(panel);
    harness.get_by_label("Agent (2 calls)");
    harness.get_by_label_contains("→ No symbol `helper`.");
    harness
        .get_by_label("  1 min  callers  name=helper")
        .click();
    harness.run();
    assert_eq!(harness.state().action, Some(AgentPanelAction::GoTo(place)));
}

#[test]
fn following_the_agent_is_a_choice() {
    let mut harness = harness(AgentPanel::default());
    assert!(!harness.state().panel.is_following());
    harness.get_by_label("Follow the agent").click();
    harness.run();
    assert!(harness.state().panel.is_following());
}

#[test]
fn not_serving_says_why() {
    let mut panel = AgentPanel::default();
    panel.set_open(true);
    let mut harness = crate::app::fonts::test_harness()
        .with_size(egui::vec2(900.0, 600.0))
        .build_ui_state(
            |ui, state: &mut State| {
                state
                    .panel
                    .show(ui, Layout::default(), &state.serving, ("x", None));
            },
            State {
                panel,
                serving: Serving::Off("Not serving agents: another window does.".into()),
                action: None,
            },
        );
    harness.run();
    harness.get_by_label("Not serving agents: another window does.");
}

#[test]
fn old_calls_give_way_to_new_ones() {
    let mut panel = AgentPanel::default();
    for _ in 0..(MAX_EVENTS + 5) {
        panel.push(event("overview", None, false));
    }
    assert_eq!(panel.events.len(), MAX_EVENTS);
    assert_eq!(panel.calls, MAX_EVENTS + 5);
}

#[test]
fn ages_read_plainly() {
    assert_eq!(age(Duration::from_secs(3)), "now");
    assert_eq!(age(Duration::from_secs(42)), "42 s");
    assert_eq!(age(Duration::from_secs(150)), "2 min");
    assert_eq!(age(Duration::from_secs(7_300)), "2 h");
}

fn verdict(rule: &str, is_injection: bool) -> VerdictChange {
    VerdictChange {
        file: "m.c".into(),
        line: 1,
        rule: rule.into(),
        status: Some(TriageStatus::FalsePositive),
        note: String::new(),
        is_injection,
    }
}

#[test]
fn approve_all_clears_the_queue_in_one_click() {
    let mut panel = AgentPanel::default();
    assert!(panel.queue_approval(verdict("a", false)));
    panel.queue_approval(verdict("b", false));
    panel.queue_approval(verdict("prompt-injection", true));
    let mut harness = harness(panel);
    harness.get_by_label_contains("3 agent verdicts waiting");
    harness.get_by_label("Approve all (3)").click();
    harness.run();
    match harness.state().action.clone() {
        Some(AgentPanelAction::ApplyVerdicts(v)) => assert_eq!(v.len(), 3),
        other => panic!("{other:?}"),
    }
}

#[test]
fn approve_all_but_injections_keeps_the_injection_ones() {
    let mut panel = AgentPanel::default();
    panel.queue_approval(verdict("a", false));
    panel.queue_approval(verdict("prompt-injection", true));
    let mut harness = harness(panel);
    harness
        .get_by_label_contains("Approve all but 1 injection")
        .click();
    harness.run();
    match harness.state().action.clone() {
        Some(AgentPanelAction::ApplyVerdicts(v)) => {
            assert_eq!(v.len(), 1);
            assert!(!v[0].is_injection);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_full_queue_is_reported_not_silently_dropped() {
    let mut panel = AgentPanel::default();
    for _ in 0..MAX_PENDING {
        assert!(panel.queue_approval(verdict("x", false)));
    }
    assert!(
        !panel.queue_approval(verdict("x", false)),
        "the cap is reported"
    );
}

#[test]
fn the_transcript_records_each_step_of_an_in_app_run() {
    let mut panel = AgentPanel::default();
    panel.push_in_app(AgentStep::Said("looking".into()));
    panel.push_in_app(AgentStep::Running {
        name: "find_symbol".into(),
        arguments: "{}".into(),
    });
    panel.push_in_app(AgentStep::Result {
        name: "find_symbol".into(),
        is_error: false,
        text: "ok\nmore".into(),
    });
    assert_eq!(panel.in_app_log.len(), 3);
    assert!(panel.in_app_log[2].1.contains("find_symbol → ok"));
}

#[test]
fn the_transcript_is_shown_in_the_pane() {
    let mut panel = AgentPanel::default();
    // An action tool: shown in the transcript but not the analysis breakdown,
    // so the label appears exactly once.
    panel.push_in_app(AgentStep::Running {
        name: "run_command".into(),
        arguments: "{}".into(),
    });
    let harness = harness(panel);
    // The step is rendered in the pane (inside the resizable transcript),
    // not left to an external terminal.
    harness.get_by_label_contains("run_command");
}

fn running(name: &str) -> AgentStep {
    AgentStep::Running {
        name: name.into(),
        arguments: "{}".into(),
    }
}

#[test]
fn the_tool_usage_summary_counts_reuse_versus_source_reads() {
    let mut panel = AgentPanel::default();
    for name in [
        "findings",
        "input_paths",
        "findings",
        "function_source",
        "run_command",
    ] {
        panel.push_in_app(running(name));
    }
    let usage = panel.in_app_tool_usage();
    assert_eq!(usage.analysis, 3, "findings x2 + input_paths x1");
    assert_eq!(usage.source, 1, "function_source");
    assert_eq!(usage.action, 1, "run_command");
    // fuzz_harness counts as reuse of a SrcWranglr feature, not an action.
    panel.push_in_app(running("fuzz_harness"));
    assert_eq!(panel.in_app_tool_usage().analysis, 4);
    // The most-used analysis tool leads the breakdown.
    assert_eq!(
        panel.in_app_tool_usage().analysis_breakdown.first(),
        Some(&("findings".to_string(), 2))
    );
}

#[test]
fn a_fresh_run_clears_the_tool_usage_tally() {
    let mut panel = AgentPanel::default();
    panel.push_in_app(running("findings"));
    assert_eq!(panel.in_app_tool_usage().analysis, 1);
    panel.set_in_app_running(true);
    assert_eq!(
        panel.in_app_tool_usage().analysis,
        0,
        "a new run starts the tally fresh"
    );
}

#[test]
fn the_reuse_summary_is_shown_in_the_pane() {
    let mut panel = AgentPanel::default();
    panel.push_in_app(running("findings"));
    let harness = harness(panel);
    harness.get_by_label_contains("Reused SrcWranglr analysis");
}

#[test]
fn finishing_denies_commands_still_waiting() {
    let (reply, answer) = std::sync::mpsc::sync_channel(1);
    let mut panel = AgentPanel::default();
    panel.set_in_app_running(true);
    panel.queue_command("echo hi".into(), reply);
    panel.finish_in_app(&Ok(String::new()));
    assert_eq!(answer.try_recv(), Ok(false), "a waiting command is denied");
    assert!(!panel.in_app_running);
}

#[test]
fn approving_a_command_in_the_pane_lets_the_agent_run_it() {
    let (reply, answer) = std::sync::mpsc::sync_channel(1);
    let mut panel = AgentPanel::default();
    panel.set_in_app_running(true);
    panel.queue_command("echo hi".into(), reply);
    let mut harness = harness(panel);
    harness.get_by_label_contains("Approve").click();
    harness.run();
    assert_eq!(answer.try_recv(), Ok(true));
}

#[test]
fn denying_a_command_in_the_pane_tells_the_agent_no() {
    let (reply, answer) = std::sync::mpsc::sync_channel(1);
    let mut panel = AgentPanel::default();
    panel.set_in_app_running(true);
    panel.queue_command("rm -rf /".into(), reply);
    let mut harness = harness(panel);
    harness.get_by_label_contains("Deny").click();
    harness.run();
    assert_eq!(answer.try_recv(), Ok(false));
}

#[test]
fn stop_agent_is_reachable_while_a_run_is_going() {
    let mut panel = AgentPanel::default();
    panel.set_in_app_running(true);
    let mut harness = harness(panel);
    harness.get_by_label_contains("Stop agent").click();
    harness.run();
    assert!(matches!(
        harness.state().action,
        Some(AgentPanelAction::StopInApp)
    ));
}

#[test]
fn stop_agent_is_hidden_when_no_run_is_going() {
    let panel = AgentPanel::default();
    let harness = harness(panel);
    assert!(
        harness.query_by_label_contains("Stop agent").is_none(),
        "no Stop button without a run"
    );
}

#[test]
fn starting_a_fresh_run_clears_the_previous_transcript() {
    let mut panel = AgentPanel::default();
    panel.push_in_app(AgentStep::Done("old run with a 401".into()));
    assert!(!panel.in_app_log.is_empty());
    panel.set_in_app_running(true);
    assert!(panel.in_app_log.is_empty(), "a fresh run starts clean");
}
