//! Cross-file resolution: merge per-file declarations into one symbol table
//! and turn name-based references into typed edges.
//!
//! Resolution order for a reference made from `a::B::m` to `x`:
//! 1. enclosing scopes, innermost first: `a::B::x` (then `a::B`'s base
//!    classes), `a::x`, `x`
//! 2. for `Lookup::ScopedOrUnique` only: the single symbol whose qualified
//!    name ends in `x`, if exactly one exists. Ambiguous names stay unlinked.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use super::extract::{Decl, FileFacts, Lookup, Reference};
use super::strings;
use crate::model::{
    CodeGraph, Edge, EdgeKind, ExternalCall, FunctionMetrics, Occurrence, OccurrenceRole, Symbol,
    SymbolId, SymbolKind,
};

/// A reference whose source and target symbols are both known.
struct Resolved<'f> {
    reference: &'f Reference,
    from: SymbolId,
    to: SymbolId,
}

/// One parsed file, ready for resolution.
pub struct SourceFile {
    /// Project-relative path, used as the display name and code-view key.
    pub path: String,
    pub contents: String,
    pub facts: FileFacts,
}

pub fn build_graph(mut files: Vec<SourceFile>) -> CodeGraph {
    let merged = merge_decls(&files);
    let symbols = assign_symbols(&merged);
    let by_qname: HashMap<&str, SymbolId> = symbols
        .values()
        .map(|s| (s.qualified_name.as_str(), s.id))
        .collect();

    let refs: Vec<&Reference> = files.iter().flat_map(|f| &f.facts.refs).collect();
    let bases = resolve_bases(&refs, &symbols, &by_qname);
    let mut resolver = MemoResolver::new(Resolver::new(&symbols, &by_qname, &bases));
    let resolved: Vec<Vec<Resolved>> = files
        .iter()
        .map(|f| resolve_file(&f.facts, &by_qname, &mut resolver))
        .collect();

    let containment = symbols.values().filter_map(|s| {
        let parent = by_qname.get(parent_of(&s.qualified_name)?)?;
        Some(Edge {
            from: *parent,
            to: s.id,
            kind: EdgeKind::Contains,
        })
    });
    let references = resolved.iter().flatten().map(|r| Edge {
        from: r.from,
        to: r.to,
        kind: r.reference.kind,
    });
    let edges = dedupe_edges(containment.chain(references), &symbols);
    let occurrences = files
        .iter()
        .zip(&resolved)
        .filter_map(|(file, refs)| {
            let list = file_occurrences(&file.facts, refs, &by_qname);
            (!list.is_empty()).then(|| (file.path.clone(), list))
        })
        .collect();

    let external_calls = files
        .iter()
        .filter_map(|file| {
            let calls = external_calls_in(&file.facts, &by_qname, &mut resolver);
            (!calls.is_empty()).then(|| (file.path.clone(), calls))
        })
        .collect();

    let functions = function_metrics(&files, &by_qname);
    let (strings, strings_cut_short) = strings::by_file(&mut files, &functions);
    let file_contents = files.into_iter().map(|f| (f.path, f.contents)).collect();
    CodeGraph {
        symbols,
        edges,
        file_contents,
        occurrences,
        external_calls,
        functions,
        strings,
        strings_cut_short,
    }
}

/// Each function definition's metrics, in file order.
fn function_metrics(
    files: &[SourceFile],
    by_qname: &HashMap<&str, SymbolId>,
) -> Vec<FunctionMetrics> {
    files
        .iter()
        .flat_map(|file| {
            file.facts.functions.iter().filter_map(|f| {
                Some(FunctionMetrics {
                    symbol: *by_qname.get(f.qualified_name.as_str())?,
                    file: file.path.clone(),
                    line: f.line,
                    lines: f.lines,
                    complexity: f.complexity,
                    signature: f.signature.clone(),
                })
            })
        })
        .collect()
}

fn resolve_file<'f>(
    facts: &'f FileFacts,
    by_qname: &HashMap<&str, SymbolId>,
    resolver: &mut MemoResolver<'_, 'f>,
) -> Vec<Resolved<'f>> {
    facts
        .refs
        .iter()
        .filter_map(|reference| {
            let from = *by_qname.get(&*reference.from)?;
            let to = resolver.resolve(reference)?;
            Some(Resolved {
                reference,
                from,
                to,
            })
        })
        .collect()
}

/// Plain function calls in one file that resolve to no project symbol,
/// sorted by position.
fn external_calls_in<'f>(
    facts: &'f FileFacts,
    by_qname: &HashMap<&str, SymbolId>,
    resolver: &mut MemoResolver<'_, 'f>,
) -> Vec<ExternalCall> {
    let mut calls: Vec<ExternalCall> = facts
        .refs
        .iter()
        .filter_map(|r| {
            let site = r.free_call.as_ref()?;
            let caller = *by_qname.get(&*r.from)?;
            let span = r.span?;
            resolver.resolve(r).is_none().then(|| ExternalCall {
                caller,
                name: r.target.clone(),
                span,
                extent: site.extent.unwrap_or(span),
                args: site.args.clone(),
            })
        })
        .collect();
    calls.sort_by_key(|c| c.span);
    calls
}

/// Declared names and resolved references in one file, sorted by position.
/// Unlike edges, self-references (recursion) and uses of an enclosing class
/// are kept: they are still clickable names in the source.
fn file_occurrences(
    facts: &FileFacts,
    refs: &[Resolved],
    by_qname: &HashMap<&str, SymbolId>,
) -> Vec<Occurrence> {
    let declarations = facts.decls.iter().filter_map(|d| {
        Some(Occurrence {
            span: d.name_span?,
            symbol: *by_qname.get(d.qualified_name.as_str())?,
            role: OccurrenceRole::Declaration,
            context: None,
        })
    });
    let references = refs.iter().filter_map(|r| {
        Some(Occurrence {
            span: r.reference.span?,
            symbol: r.to,
            role: OccurrenceRole::Reference(r.reference.kind),
            context: Some(r.from),
        })
    });
    let mut list: Vec<Occurrence> = declarations.chain(references).collect();
    list.sort_by_key(|o| (o.span, o.symbol));
    list.dedup_by_key(|o| (o.span, o.symbol));
    list
}

fn parent_of(qualified: &str) -> Option<&str> {
    qualified.rsplit_once("::").map(|(parent, _)| parent)
}

fn last_segment(qualified: &str) -> &str {
    qualified
        .rsplit_once("::")
        .map_or(qualified, |(_, name)| name)
}

/// One entry per qualified name. Definitions win over declarations; among
/// equals, the first file (in input order) wins.
fn merge_decls(files: &[SourceFile]) -> BTreeMap<&str, (&Decl, &str)> {
    let mut merged: BTreeMap<&str, (&Decl, &str)> = BTreeMap::new();
    for file in files {
        for decl in &file.facts.decls {
            let entry = merged
                .entry(decl.qualified_name.as_str())
                .or_insert((decl, &file.path));
            if decl.is_definition && !entry.0.is_definition {
                *entry = (decl, &file.path);
            }
        }
    }
    merged
}

/// Ids follow sorted qualified-name order, so they're stable across runs.
/// Functions whose parent is a class/struct (out-of-line definitions such as
/// `void Foo::bar() {}`) become methods.
fn assign_symbols(merged: &BTreeMap<&str, (&Decl, &str)>) -> HashMap<SymbolId, Symbol> {
    let is_type = |name: &str| {
        merged
            .get(name)
            .is_some_and(|(d, _)| matches!(d.kind, SymbolKind::Class | SymbolKind::Struct))
    };
    (0..)
        .zip(merged.iter())
        .map(|(id, (qualified, (decl, file)))| {
            let is_member_function =
                decl.kind == SymbolKind::Function && parent_of(qualified).is_some_and(is_type);
            let kind = if is_member_function {
                SymbolKind::Method
            } else {
                decl.kind
            };
            let symbol = Symbol {
                id,
                name: last_segment(qualified).to_string(),
                qualified_name: qualified.to_string(),
                kind,
                file: file.to_string(),
                line: decl.line,
            };
            (id, symbol)
        })
        .collect()
}

/// Class → resolved direct base classes. Bases resolve via scopes and
/// unique names only (no base-class lookup, which needs this very map).
fn resolve_bases(
    refs: &[&Reference],
    symbols: &HashMap<SymbolId, Symbol>,
    by_qname: &HashMap<&str, SymbolId>,
) -> HashMap<SymbolId, Vec<SymbolId>> {
    let no_bases = HashMap::new();
    let resolver = Resolver::new(symbols, by_qname, &no_bases);
    let mut bases: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
    for r in refs.iter().filter(|r| r.kind == EdgeKind::Inherits) {
        let (Some(from), Some(base)) = (by_qname.get(&*r.from), resolver.resolve(r)) else {
            continue;
        };
        if *from != base {
            bases.entry(*from).or_default().push(base);
        }
    }
    bases
}

/// Drops duplicates, self-loops, and references from a symbol to one of its
/// own enclosing scopes (e.g. a constructor "using" its class).
fn dedupe_edges(
    edges: impl Iterator<Item = Edge>,
    symbols: &HashMap<SymbolId, Symbol>,
) -> Vec<Edge> {
    let is_ancestor = |ancestor: SymbolId, of: SymbolId| {
        let (Some(a), Some(o)) = (symbols.get(&ancestor), symbols.get(&of)) else {
            return false;
        };
        o.qualified_name
            .strip_prefix(a.qualified_name.as_str())
            .is_some_and(|rest| rest.starts_with("::"))
    };
    let mut seen = HashSet::new();
    edges
        .filter(|e| e.from != e.to)
        .filter(|e| e.kind == EdgeKind::Contains || !is_ancestor(e.to, e.from))
        .filter(|e| seen.insert((e.from, e.to, e.kind)))
        .collect()
}

/// Caches lookups: the same name is usually referenced many times from the
/// same symbol, and resolution only depends on `(from, target, lookup)`.
struct MemoResolver<'a, 'f> {
    resolver: Resolver<'a>,
    cache: HashMap<(&'f str, &'f str, Lookup), Option<SymbolId>>,
}

impl<'a, 'f> MemoResolver<'a, 'f> {
    fn new(resolver: Resolver<'a>) -> Self {
        Self {
            resolver,
            cache: HashMap::new(),
        }
    }

    fn resolve(&mut self, r: &'f Reference) -> Option<SymbolId> {
        let key = (&*r.from, r.target.as_str(), r.lookup);
        *self
            .cache
            .entry(key)
            .or_insert_with(|| self.resolver.resolve(r))
    }
}

struct Resolver<'a> {
    symbols: &'a HashMap<SymbolId, Symbol>,
    by_qname: &'a HashMap<&'a str, SymbolId>,
    by_name: HashMap<&'a str, Vec<SymbolId>>,
    bases: &'a HashMap<SymbolId, Vec<SymbolId>>,
}

impl<'a> Resolver<'a> {
    fn new(
        symbols: &'a HashMap<SymbolId, Symbol>,
        by_qname: &'a HashMap<&'a str, SymbolId>,
        bases: &'a HashMap<SymbolId, Vec<SymbolId>>,
    ) -> Self {
        let mut by_name: HashMap<&str, Vec<SymbolId>> = HashMap::new();
        for symbol in symbols.values() {
            by_name
                .entry(symbol.name.as_str())
                .or_default()
                .push(symbol.id);
        }
        Self {
            symbols,
            by_qname,
            by_name,
            bases,
        }
    }

    fn resolve(&self, r: &Reference) -> Option<SymbolId> {
        self.resolve_scoped(&r.from, &r.target)
            .or_else(|| match r.lookup {
                Lookup::ScopedOrUnique => self.resolve_unique(&r.target),
                Lookup::Scoped => None,
            })
    }

    /// `target` looked up in each enclosing scope of `from`, innermost first,
    /// including members inherited by enclosing classes.
    fn resolve_scoped(&self, from: &str, target: &str) -> Option<SymbolId> {
        let mut scope = parent_of(from);
        while let Some(s) = scope {
            if let Some(id) = self.by_qname.get(format!("{s}::{target}").as_str()) {
                return Some(*id);
            }
            let inherited = self
                .by_qname
                .get(s)
                .and_then(|id| self.find_in_bases(*id, target));
            if inherited.is_some() {
                return inherited;
            }
            scope = parent_of(s);
        }
        // Global scope, then `Type::member` where the member is inherited.
        self.by_qname
            .get(target)
            .copied()
            .or_else(|| self.resolve_inherited_member(from, target))
    }

    /// `Derived::bump` → `Base::bump` when `bump` is declared in a base of `Derived`.
    fn resolve_inherited_member(&self, from: &str, target: &str) -> Option<SymbolId> {
        let (qualifier, member) = target.rsplit_once("::")?;
        let owner = self
            .resolve_scoped(from, qualifier)
            .or_else(|| self.resolve_unique(qualifier))?;
        self.find_in_bases(owner, member)
    }

    /// Breadth-first through base classes (guards against inheritance cycles).
    fn find_in_bases(&self, class: SymbolId, member: &str) -> Option<SymbolId> {
        let mut queue: VecDeque<SymbolId> = self
            .bases
            .get(&class)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        let mut visited = HashSet::from([class]);
        while let Some(base) = queue.pop_front() {
            if !visited.insert(base) {
                continue;
            }
            let base_name = &self.symbols.get(&base)?.qualified_name;
            if let Some(id) = self.by_qname.get(format!("{base_name}::{member}").as_str()) {
                return Some(*id);
            }
            queue.extend(self.bases.get(&base).into_iter().flatten());
        }
        None
    }

    /// The only symbol named `target` (or ending in `::target`), if unique.
    fn resolve_unique(&self, target: &str) -> Option<SymbolId> {
        let suffix = format!("::{target}");
        let mut matches = self.by_name.get(last_segment(target))?.iter().filter(|id| {
            self.symbols
                .get(id)
                .is_some_and(|s| s.qualified_name == target || s.qualified_name.ends_with(&suffix))
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(*first)
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
