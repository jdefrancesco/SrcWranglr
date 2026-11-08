//! What the Matching pane knows: the shapes of the project's code and the
//! look-alike groups among them, the functions like one picked, and each
//! function's behaviors. All of it is worked out in the background, once
//! asked for, so a big project never holds up the window.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use super::background::BackgroundJob;
use crate::analysis::behavior::{direct_behaviors, reached_behaviors, Behavior, DirectBehaviors};
use crate::analysis::lookalike::{self, Group, Shapes, UnitKind};
use crate::model::{CodeGraph, SymbolId};

/// Names in a group's heading; the rest are counted.
const MAX_NAMES_SHOWN: usize = 3;
/// A group's heading is cut to this many characters: collapsing headings
/// don't wrap, and a long one would widen the pane.
const MAX_HEADING_CHARS: usize = 64;

/// The project's look-alike code.
pub struct LookAlikes {
    pub shapes: Arc<Shapes>,
    pub groups: Vec<Group>,
    /// Each group's heading, made once.
    pub headings: Vec<String>,
    /// More groups than listed.
    pub is_cut_short: bool,
}

/// A background step and what it came to.
pub enum Work<T> {
    Running(BackgroundJob<Option<T>>),
    Done(Arc<T>),
    Failed(String),
}

impl<T: Send + 'static> Work<T> {
    fn start(
        what: &'static str,
        work: impl FnOnce(&std::sync::atomic::AtomicBool) -> Option<T> + Send + 'static,
        ctx: &egui::Context,
    ) -> Self {
        Work::Running(BackgroundJob::spawn(
            what,
            move |stop| Ok(work(stop)),
            ctx.clone(),
        ))
    }

    /// Takes in the result once there. Whether it just came.
    fn poll(&mut self) -> bool {
        let Work::Running(job) = self else {
            return false;
        };
        let Some(result) = job.poll() else {
            return false;
        };
        *self = match result {
            Ok(Some(done)) => Work::Done(Arc::new(done)),
            Ok(None) => Work::Failed("Stopped.".into()),
            Err(error) => Work::Failed(error),
        };
        true
    }

    pub fn done(&self) -> Option<&Arc<T>> {
        match self {
            Work::Done(done) => Some(done),
            _ => None,
        }
    }
}

/// Starts reading the look-alike code of `graph`.
pub fn read_look_alikes(graph: Arc<CodeGraph>, ctx: &egui::Context) -> Work<LookAlikes> {
    Work::start(
        "Finding look-alike code",
        move |stop| {
            let shapes = lookalike::shapes_of(&graph, stop)?;
            let (groups, is_cut_short) = lookalike::groups(&shapes);
            let headings = groups.iter().map(|g| heading(g, &shapes, &graph)).collect();
            Some(LookAlikes {
                shapes: Arc::new(shapes),
                groups,
                headings,
                is_cut_short,
            })
        },
        ctx,
    )
}

/// "3 functions · 52 tokens — parse_a, parse_b, parse_c and 2 more".
fn heading(group: &Group, shapes: &Shapes, graph: &CodeGraph) -> String {
    let what = match group.kind {
        UnitKind::Function => "functions",
        UnitKind::Block => "blocks",
    };
    let mut seen = HashSet::new();
    let names: Vec<String> = group
        .units
        .iter()
        .map(|&i| unit_name(shapes.units[i].symbol, graph))
        .filter(|name| seen.insert(name.clone()))
        .collect();
    let more = match names.len().saturating_sub(MAX_NAMES_SHOWN) {
        0 => String::new(),
        n => format!(" and {n} more"),
    };
    let shown = names
        .iter()
        .take(MAX_NAMES_SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!(
        "{} {what} · {} tokens — {shown}{more}",
        group.units.len(),
        group.tokens
    );
    cut(&text, MAX_HEADING_CHARS)
}

/// The function `symbol` names, or "(file scope)".
pub fn unit_name(symbol: Option<SymbolId>, graph: &CodeGraph) -> String {
    symbol
        .and_then(|id| graph.symbols.get(&id))
        .map_or_else(|| "(file scope)".to_string(), |s| s.qualified_name.clone())
}

/// `text` cut to `max` characters, ending in `…` when cut.
pub fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// The functions like one: which, and their ranking once worked out.
pub struct Like {
    pub symbol: SymbolId,
    pub found: Option<Work<Vec<(usize, f32)>>>,
}

impl Like {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            found: None,
        }
    }

    /// Starts ranking once the shapes are read; takes in the ranking.
    pub fn poll(&mut self, shapes: Option<&Arc<Shapes>>, ctx: &egui::Context) {
        match (&mut self.found, shapes) {
            (Some(work), _) => {
                work.poll();
            }
            (None, Some(shapes)) => {
                let (shapes, symbol) = (Arc::clone(shapes), self.symbol);
                self.found = Some(Work::start(
                    "Ranking code like a function",
                    move |_| Some(lookalike::like(&shapes, symbol)),
                    ctx,
                ));
            }
            (None, None) => {}
        }
    }
}

/// Every function's behaviors, directly and through its calls.
pub struct Behaviors {
    pub direct: DirectBehaviors,
    pub reached: HashMap<Behavior, HashSet<SymbolId>>,
    /// How many functions have each behavior: directly, through calls.
    pub counts: [BTreeMap<Behavior, usize>; 2],
}

/// Starts working out the behaviors of `graph`'s functions.
pub fn read_behaviors(graph: Arc<CodeGraph>, ctx: &egui::Context) -> Work<Behaviors> {
    Work::start(
        "Tagging functions by behavior",
        move |_| Some(Behaviors::of(&graph)),
        ctx,
    )
}

impl Behaviors {
    pub fn of(graph: &CodeGraph) -> Self {
        let direct = direct_behaviors(graph);
        let reached = reached_behaviors(graph, &direct);
        let direct_counts = Behavior::ALL
            .into_iter()
            .map(|b| {
                let count = direct.values().filter(|f| f.contains_key(&b)).count();
                (b, count)
            })
            .collect();
        let reached_counts = Behavior::ALL
            .into_iter()
            .map(|b| (b, reached.get(&b).map_or(0, HashSet::len)))
            .collect();
        Self {
            direct,
            reached,
            counts: [direct_counts, reached_counts],
        }
    }

    /// How many functions have each behavior.
    pub fn counts(&self, through_calls: bool) -> &BTreeMap<Behavior, usize> {
        &self.counts[usize::from(through_calls)]
    }

    /// The functions with every one of `wanted`, sorted by name.
    pub fn matching(
        &self,
        graph: &CodeGraph,
        wanted: &BTreeSet<Behavior>,
        through_calls: bool,
    ) -> Vec<SymbolId> {
        if wanted.is_empty() {
            return Vec::new();
        }
        let has = |id: &SymbolId, behavior: &Behavior| match through_calls {
            true => self.reached.get(behavior).is_some_and(|s| s.contains(id)),
            false => self
                .direct
                .get(id)
                .is_some_and(|found| found.contains_key(behavior)),
        };
        let mut found: Vec<SymbolId> = graph
            .symbols
            .keys()
            .filter(|id| wanted.iter().all(|b| has(id, b)))
            .copied()
            .collect();
        found.sort_by_cached_key(|id| {
            graph
                .symbols
                .get(id)
                .map(|s| s.qualified_name.to_lowercase())
        });
        found
    }

    /// Why `id` has `behavior`: the calls it makes that show it, or that
    /// it has it through the functions it calls.
    pub fn why(&self, id: SymbolId, behavior: Behavior) -> String {
        match self.direct.get(&id).and_then(|found| found.get(&behavior)) {
            Some(calls) => calls
                .iter()
                .map(|c| format!("{c}()"))
                .collect::<Vec<_>>()
                .join(", "),
            None => "through the functions it calls".to_string(),
        }
    }
}

/// Polls `work` if any. Whether it just finished.
pub fn poll<T: Send + 'static>(work: &mut Option<Work<T>>) -> bool {
    work.as_mut().is_some_and(Work::poll)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    const SERVER: &str = "void copy_in(char *d, const char *s, int n) { memcpy(d, s, n); }\n\
void handle(int fd) { char b[8], o[8]; int n = recv(fd, b, 8, 0); copy_in(o, b, n); }\n\
void log_it(const char *m) { printf(\"%s\", m); }\n";

    fn named(graph: &CodeGraph, ids: &[SymbolId]) -> Vec<String> {
        ids.iter()
            .map(|id| graph.symbols[id].name.clone())
            .collect()
    }

    fn finished<T: Send + 'static>(mut work: Work<T>) -> Arc<T> {
        let started = std::time::Instant::now();
        while !work.poll() {
            assert!(started.elapsed().as_secs() < 30, "never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Arc::clone(work.done().expect("done"))
    }

    #[test]
    fn functions_with_every_behavior_wanted_directly_or_through_calls() {
        let graph = graph_of_files(&[("s.c", SERVER)]);
        let behaviors = Behaviors::of(&graph);
        let both = BTreeSet::from([Behavior::ReadsInput, Behavior::CopiesMemory]);
        assert!(behaviors.matching(&graph, &both, false).is_empty());
        let found = behaviors.matching(&graph, &both, true);
        assert_eq!(named(&graph, &found), ["handle"]);
        let handle = found[0];
        assert_eq!(behaviors.why(handle, Behavior::ReadsInput), "recv()");
        assert_eq!(
            behaviors.why(handle, Behavior::CopiesMemory),
            "through the functions it calls"
        );
        assert!(behaviors
            .matching(&graph, &BTreeSet::new(), true)
            .is_empty());
    }

    #[test]
    fn counts_each_behavior() {
        let graph = graph_of_files(&[("s.c", SERVER)]);
        let behaviors = Behaviors::of(&graph);
        assert_eq!(behaviors.counts(false)[&Behavior::CopiesMemory], 1);
        assert_eq!(behaviors.counts(false)[&Behavior::FormatsText], 1);
        assert_eq!(behaviors.counts(true)[&Behavior::CopiesMemory], 2);
    }

    #[test]
    fn look_alikes_are_read_grouped_and_headed_in_the_background() {
        let source = "int a(int *p, int n) { int t = 0; for (int i = 0; i < n; i++) { t += p[i] * \
            2; if (t > 9) { t = t - 9; } } return t + n; }\nint b(int *q, int m) { int s = 0; for \
            (int j = 0; j < m; j++) { s += q[j] * 5; if (s > 7) { s = s - 4; } } return s + m; }\n";
        let graph = Arc::new(graph_of_files(&[("a.c", source)]));
        let ctx = egui::Context::default();
        let found = finished(read_look_alikes(Arc::clone(&graph), &ctx));
        assert_eq!(found.groups.len(), 1);
        assert_eq!(found.headings, ["2 functions · 61 tokens — a, b"]);
        let behaviors = finished(read_behaviors(graph, &ctx));
        assert!(behaviors.counts(false).values().all(|&n| n == 0));
    }

    #[test]
    fn a_long_heading_is_cut_and_names_counted_once() {
        assert_eq!(cut("abcdef", 3), "abc…");
        assert_eq!(cut("abc", 3), "abc");
    }
}
