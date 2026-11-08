//! Per-file extraction: parse C/C++ with tree-sitter and collect symbol
//! declarations plus *unresolved* references (by name, as written).
//!
//! This is syntax-only. Names are turned into symbols later by `resolve`,
//! which uses scope chains, base classes, and local variable types as
//! heuristics, so overloads collapse into a single symbol.

use std::collections::HashMap;
use std::sync::Arc;

use tree_sitter::{Node, Parser};

use super::complexity;
use super::signature;
use super::strings::{self, FileStrings};
use super::syntax::{
    clean_name, declarator_name, declared_name, field_children, find_function_declarator,
    is_class_like, is_function_pointer, join, line_of, name_span, named_children, segments,
    span_of, text, type_name,
};

use crate::model::{EdgeKind, Signature, Span, SymbolKind};

/// Guards recursion through nested namespaces/classes on pathological input.
const MAX_DECL_DEPTH: usize = 256;
/// Guards unwrapping chains like `pointer_declarator > pointer_declarator > ...`.
pub(super) const MAX_DECLARATOR_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub qualified_name: String,
    pub kind: SymbolKind,
    /// 1-based line of the declaration.
    pub line: u32,
    /// `true` for definitions (bodies), `false` for prototypes/declarations.
    pub is_definition: bool,
    /// Where the declared name itself is written.
    pub name_span: Option<Span>,
}

/// How a reference target may be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lookup {
    /// Only via the referencing symbol's enclosing scopes (and their bases).
    Scoped,
    /// Scoped first, then any symbol whose name uniquely matches.
    ScopedOrUnique,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Reference {
    /// Qualified name of the referencing symbol.
    /// Shared between all references made from the same symbol.
    pub from: Arc<str>,
    /// Target name as written, possibly qualified (`a::b`).
    pub target: String,
    pub kind: EdgeKind,
    pub lookup: Lookup,
    /// Where the target's name is written (`None` if it spans lines).
    pub span: Option<Span>,
    /// Set for a plain function call, `f(...)` or `ns::f(...)` (not
    /// `obj.f(...)`): if it doesn't resolve, it calls outside the project.
    pub free_call: Option<CallSite>,
}

/// What a plain function call passes, and where it is written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallSite {
    /// Each argument's source text (long ones cut short).
    pub args: Vec<String>,
    /// From the name to the closing parenthesis when that is on one line,
    /// else just the name.
    pub extent: Option<Span>,
}

/// Arguments of a call kept for analysis, and how much of each.
const MAX_CALL_ARGS: usize = 8;
const MAX_ARG_CHARS: usize = 1024;

/// Size and branching of one function definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionFacts {
    pub qualified_name: String,
    /// 1-based line where the definition starts.
    pub line: u32,
    pub lines: u32,
    pub complexity: u32,
    pub signature: Signature,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct FileFacts {
    pub decls: Vec<Decl>,
    pub refs: Vec<Reference>,
    pub functions: Vec<FunctionFacts>,
    /// The string literals written in the file.
    pub strings: FileStrings,
}

pub struct Extractor {
    parser: Parser,
    /// How many more strings the project may keep.
    strings_left: usize,
}

impl Extractor {
    pub fn new() -> Result<Self, String> {
        Self::with_string_budget(strings::MAX_STRINGS_TOTAL)
    }

    /// An extractor that keeps at most `budget` strings over all the files
    /// it reads.
    pub fn with_string_budget(budget: usize) -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .map_err(|e| e.to_string())?;
        Ok(Self {
            parser,
            strings_left: budget,
        })
    }

    /// Returns `None` only if tree-sitter gives up on the input entirely;
    /// syntax errors still yield whatever could be recovered.
    pub fn extract(&mut self, source: &str) -> Option<FileFacts> {
        let tree = self.parser.parse(source, None)?;
        let mut walker = Walker {
            src: source,
            facts: FileFacts::default(),
            last_from: None,
        };
        walker.visit_children(tree.root_node(), &[], false, 0);
        let limit = self.strings_left.min(strings::MAX_STRINGS_PER_FILE);
        let strings = strings::collect(tree.root_node(), source, limit);
        self.strings_left = self.strings_left.saturating_sub(strings.strings.len());
        Some(FileFacts {
            strings,
            ..walker.facts
        })
    }
}

// ---------------------------------------------------------------------------
// Declaration walk (namespaces, classes, functions, fields)
// ---------------------------------------------------------------------------

struct Walker<'s> {
    src: &'s str,
    facts: FileFacts,
    /// The most recent referencing symbol's name, reused for its references.
    last_from: Option<Arc<str>>,
}

impl Walker<'_> {
    /// Records a reference written at `at` (a name node). Every occurrence
    /// is kept: each one is a clickable location in the code view.
    fn add_ref(&mut self, from: &str, target: String, kind: EdgeKind, lookup: Lookup, at: Node) {
        self.push_ref(from, target, kind, lookup, at, None);
    }

    /// A plain function call `target(...)` made from `from`; `at` is the
    /// name, `call` the whole call expression.
    fn add_free_call(&mut self, from: &str, target: String, at: Node, call: Node) {
        let (kind, lookup) = (EdgeKind::Calls, Lookup::ScopedOrUnique);
        let site = call_site(at, call, self.src);
        self.push_ref(from, target, kind, lookup, at, Some(site));
    }

    fn push_ref(
        &mut self,
        from: &str,
        target: String,
        kind: EdgeKind,
        lookup: Lookup,
        at: Node,
        free_call: Option<CallSite>,
    ) {
        if target.is_empty() || from.is_empty() {
            return;
        }
        let from = self.shared_from(from);
        self.facts.refs.push(Reference {
            from,
            target,
            kind,
            lookup,
            span: name_span(at),
            free_call,
        });
    }

    /// `from` as a shared string, allocating only when it changes.
    fn shared_from(&mut self, from: &str) -> Arc<str> {
        match &self.last_from {
            Some(last) if **last == *from => Arc::clone(last),
            _ => {
                let shared: Arc<str> = Arc::from(from);
                self.last_from = Some(Arc::clone(&shared));
                shared
            }
        }
    }

    fn visit_children(&mut self, node: Node, scope: &[String], in_class: bool, depth: usize) {
        for child in named_children(node) {
            self.visit(child, scope, in_class, depth + 1);
        }
    }

    fn visit(&mut self, node: Node, scope: &[String], in_class: bool, depth: usize) {
        if depth > MAX_DECL_DEPTH {
            return;
        }
        match node.kind() {
            "namespace_definition" => self.visit_namespace(node, scope, depth),
            kind if is_class_like(kind) => self.visit_class(node, scope, None, depth),
            "function_definition" => self.visit_function(node, scope, in_class),
            "field_declaration" | "declaration" => {
                self.visit_declaration(node, scope, in_class, depth)
            }
            "type_definition" => self.visit_typedef(node, scope, depth),
            "template_declaration"
            | "linkage_specification"
            | "declaration_list"
            | "preproc_if"
            | "preproc_ifdef"
            | "preproc_else"
            | "preproc_elif"
            | "preproc_elifdef" => self.visit_children(node, scope, in_class, depth),
            _ => {}
        }
    }

    fn visit_namespace(&mut self, node: Node, scope: &[String], depth: usize) {
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| clean_name(text(n, self.src)));
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            // Anonymous namespace: members belong to the enclosing scope.
            return self.visit_children(body, scope, false, depth);
        };
        let inner: Vec<String> = scope.iter().cloned().chain(segments(&name)).collect();
        self.facts.decls.push(Decl {
            qualified_name: inner.join("::"),
            kind: SymbolKind::Namespace,
            line: line_of(node),
            is_definition: true,
            name_span: name_node.and_then(name_span),
        });
        self.visit_children(body, &inner, false, depth);
    }

    fn visit_class(
        &mut self,
        node: Node,
        scope: &[String],
        typedef_name: Option<(String, Node)>,
        depth: usize,
    ) {
        let Some(body) = node.child_by_field_name("body") else {
            return; // forward declaration or elaborated type use
        };
        let name = node
            .child_by_field_name("name")
            .and_then(|n| Some((type_name(n, self.src)?, n)))
            .or(typedef_name);
        let Some((name, name_node)) = name else {
            // Anonymous struct/union: members belong to the enclosing scope.
            return self.visit_children(body, scope, true, depth);
        };
        let inner: Vec<String> = scope.iter().cloned().chain(segments(&name)).collect();
        let qualified = inner.join("::");
        let kind = if node.kind() == "class_specifier" {
            SymbolKind::Class
        } else {
            SymbolKind::Struct
        };
        self.facts.decls.push(Decl {
            qualified_name: qualified.clone(),
            kind,
            line: line_of(node),
            is_definition: true,
            name_span: name_span(name_node),
        });

        let bases = named_children(node)
            .into_iter()
            .filter(|c| c.kind() == "base_class_clause");
        for base in bases.flat_map(named_children) {
            if let Some(base_name) = type_name(base, self.src) {
                self.add_ref(
                    &qualified,
                    base_name,
                    EdgeKind::Inherits,
                    Lookup::ScopedOrUnique,
                    base,
                );
            }
        }
        self.visit_children(body, &inner, true, depth);
    }

    fn visit_typedef(&mut self, node: Node, scope: &[String], depth: usize) {
        let Some(ty) = node
            .child_by_field_name("type")
            .filter(|t| is_class_like(t.kind()))
        else {
            return;
        };
        let alias = field_children(node, "declarator")
            .into_iter()
            .find_map(|d| declarator_name(d).map(|n| (clean_name(text(n, self.src)), n)));
        let typedef_name = if ty.child_by_field_name("name").is_none() {
            alias
        } else {
            None
        };
        self.visit_class(ty, scope, typedef_name, depth);
    }

    /// Prototypes, fields, and nested type definitions.
    fn visit_declaration(&mut self, node: Node, scope: &[String], in_class: bool, depth: usize) {
        let ty = node.child_by_field_name("type");
        if let Some(ty) = ty.filter(|t| is_class_like(t.kind())) {
            self.visit_class(ty, scope, None, depth);
        }
        let owner = join(scope, "");
        let owner = owner.trim_end_matches("::");
        for declarator in field_children(node, "declarator") {
            if let Some(func) =
                find_function_declarator(declarator).filter(|f| !is_function_pointer(*f))
            {
                if let Some((name, name_node)) = self.function_name(func) {
                    let kind = if in_class {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    let qualified_name = join(scope, &name);
                    self.facts.decls.push(Decl {
                        qualified_name,
                        kind,
                        line: line_of(node),
                        is_definition: false,
                        name_span: name_span(name_node),
                    });
                }
            } else if in_class {
                let Some(name_node) = declarator_name(declarator) else {
                    continue;
                };
                let name = declared_name(name_node, self.src);
                self.facts.decls.push(Decl {
                    qualified_name: join(scope, &name),
                    kind: SymbolKind::Field,
                    line: line_of(declarator),
                    is_definition: true,
                    name_span: name_span(name_node),
                });
                let type_ref = ty.and_then(|t| Some((type_name(t, self.src)?, t)));
                if let Some((type_ref, at)) = type_ref {
                    self.add_ref(owner, type_ref, EdgeKind::Uses, Lookup::ScopedOrUnique, at);
                }
            }
        }
    }

    /// The declared function name and the node it is written in.
    fn function_name<'t>(&self, func_declarator: Node<'t>) -> Option<(String, Node<'t>)> {
        let name_node = func_declarator
            .child_by_field_name("declarator")
            .and_then(declarator_name)?;
        let name = declared_name(name_node, self.src);
        (!name.is_empty()).then_some((name, name_node))
    }

    fn visit_function(&mut self, node: Node, scope: &[String], in_class: bool) {
        let Some(func) = node
            .child_by_field_name("declarator")
            .and_then(find_function_declarator)
        else {
            return;
        };
        let Some((name, name_node)) = self.function_name(func) else {
            return;
        };
        let qualified = join(scope, &name);
        let kind = if in_class {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        self.facts.decls.push(Decl {
            qualified_name: qualified.clone(),
            kind,
            line: line_of(node),
            is_definition: true,
            name_span: name_span(name_node),
        });
        self.facts.functions.push(FunctionFacts {
            qualified_name: qualified.clone(),
            line: line_of(node),
            lines: complexity::line_count(node),
            complexity: complexity::cyclomatic(node),
            signature: signature::of(node, func, self.src),
        });

        let initializers = named_children(node)
            .into_iter()
            .filter(|c| c.kind() == "field_initializer_list");
        let roots: Vec<Node> = node
            .child_by_field_name("type")
            .into_iter()
            .chain(func.child_by_field_name("parameters"))
            .chain(initializers)
            .chain(node.child_by_field_name("body"))
            .collect();
        BodyScanner {
            walker: self,
            from: &qualified,
            locals: HashMap::new(),
        }
        .scan(&roots);
    }
}

// ---------------------------------------------------------------------------
// Function body scan (calls, uses, local variable types)
// ---------------------------------------------------------------------------

struct BodyScanner<'w, 's> {
    walker: &'w mut Walker<'s>,
    from: &'w str,
    /// Local variable / parameter name → its declared user type, if any.
    locals: HashMap<String, Option<String>>,
}

impl BodyScanner<'_, '_> {
    fn src(&self) -> &str {
        self.walker.src
    }

    fn add(&mut self, target: String, kind: EdgeKind, lookup: Lookup, at: Node) {
        self.walker.add_ref(self.from, target, kind, lookup, at);
    }

    /// Iterative pre-order walk; deep expression trees can't overflow the stack.
    fn scan(&mut self, roots: &[Node]) {
        let mut stack: Vec<Node> = roots.iter().rev().copied().collect();
        while let Some(node) = stack.pop() {
            let children = self.visit(node);
            stack.extend(children.into_iter().rev());
        }
    }

    /// Records whatever `node` contributes and returns the children to descend into.
    fn visit<'t>(&mut self, node: Node<'t>) -> Vec<Node<'t>> {
        match node.kind() {
            "parameter_declaration"
            | "optional_parameter_declaration"
            | "declaration"
            | "for_range_loop" => {
                self.register_locals(node);
                named_children(node)
            }
            "call_expression" => self.visit_call(node),
            "field_expression" => {
                self.visit_member(node, EdgeKind::Uses);
                node.child_by_field_name("argument").into_iter().collect()
            }
            "field_initializer" => {
                let field = named_children(node)
                    .into_iter()
                    .find(|c| c.kind() == "field_identifier");
                if let Some(field) = field {
                    self.add(
                        clean_name(text(field, self.src())),
                        EdgeKind::Uses,
                        Lookup::Scoped,
                        field,
                    );
                }
                named_children(node)
                    .into_iter()
                    .filter(|c| c.kind() != "field_identifier")
                    .collect()
            }
            "identifier" => {
                let name = text(node, self.src()).to_string();
                if !self.locals.contains_key(&name) {
                    self.add(name, EdgeKind::Uses, Lookup::Scoped, node);
                }
                Vec::new()
            }
            "type_identifier" | "qualified_identifier" | "template_type" => {
                if let Some(name) = type_name(node, self.src()) {
                    self.add(name, EdgeKind::Uses, Lookup::ScopedOrUnique, node);
                }
                Vec::new()
            }
            _ => named_children(node),
        }
    }

    fn register_locals(&mut self, node: Node) {
        let ty = node
            .child_by_field_name("type")
            .and_then(|t| type_name(t, self.src()));
        for declarator in field_children(node, "declarator") {
            if let Some(name) = declarator_name(declarator) {
                self.locals
                    .insert(text(name, self.src()).to_string(), ty.clone());
            }
        }
    }

    fn visit_call<'t>(&mut self, node: Node<'t>) -> Vec<Node<'t>> {
        let arguments = node.child_by_field_name("arguments");
        let Some(function) = node.child_by_field_name("function") else {
            return arguments.into_iter().collect();
        };
        let mut descend: Vec<Node> = Vec::new();
        match function.kind() {
            "identifier" | "qualified_identifier" | "template_function" => {
                let name_node = match function.kind() {
                    "template_function" => function.child_by_field_name("name"),
                    _ => Some(function),
                };
                let name_node = name_node.unwrap_or(function);
                let name = clean_name(text(name_node, self.src()));
                if !self.locals.contains_key(&name) {
                    self.walker.add_free_call(self.from, name, name_node, node);
                }
            }
            "field_expression" => {
                self.visit_member(function, EdgeKind::Calls);
                descend.extend(function.child_by_field_name("argument"));
            }
            _ => descend.push(function),
        }
        descend.extend(arguments);
        descend
    }

    /// `obj.member` / `ptr->member` / `this->member`, typed when possible.
    fn visit_member(&mut self, node: Node, kind: EdgeKind) {
        let Some(field) = node.child_by_field_name("field") else {
            return;
        };
        let member = clean_name(text(field, self.src()));
        let receiver = node.child_by_field_name("argument");
        let receiver_text = receiver.map(|r| text(r, self.src())).unwrap_or("");

        if receiver_text == "this" {
            return self.add(member, kind, Lookup::Scoped, field);
        }
        let receiver_type = receiver
            .filter(|r| r.kind() == "identifier")
            .and_then(|_| self.locals.get(receiver_text).cloned().flatten());
        match (receiver_type, kind) {
            (Some(ty), _) => self.add(format!("{ty}::{member}"), kind, Lookup::Scoped, field),
            // Unknown receiver: a call may still be linkable by unique name;
            // a plain field access is too ambiguous to guess.
            (None, EdgeKind::Calls) => self.add(member, kind, Lookup::ScopedOrUnique, field),
            (None, _) => {}
        }
    }
}

/// The arguments and extent of the call expression `call`, whose name is
/// at `name`.
fn call_site(name: Node, call: Node, src: &str) -> CallSite {
    let args = call
        .child_by_field_name("arguments")
        .map(|list| {
            named_children(list)
                .into_iter()
                .filter(|arg| arg.kind() != "comment")
                .take(MAX_CALL_ARGS)
                .map(|arg| text(arg, src).chars().take(MAX_ARG_CHARS).collect())
                .collect()
        })
        .unwrap_or_default();
    let name_span = name_span(name);
    let extent = match (name_span, span_of(call)) {
        (Some(n), Some(c)) => Some(Span { end: c.end, ..n }),
        (n, _) => n,
    };
    CallSite { args, extent }
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
