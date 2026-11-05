//! The harness's `LLVMFuzzerTestOneInput`: a local for each parameter
//! (numbers and hand-built values first, buffers last so the final one
//! can take the rest of the input), then the call.

use std::collections::BTreeSet;

use super::{comment_safe, is_identifier, is_qualified_identifier, is_safe_type, split_name, TODO};
use crate::analysis::fuzz::params::{is_path_name, Fill, NumberKind};
use crate::analysis::fuzz::Placement;
use crate::model::{FunctionMetrics, Param};

/// Names the harness itself uses.
const RESERVED: &[&str] = &[
    "data",
    "size",
    "provider",
    "object",
    "std",
    "LLVMFuzzerTestOneInput",
];
/// Words C allows as names that C++ doesn't.
const CPP_KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "bitand",
    "bitor",
    "bool",
    "catch",
    "char8_t",
    "char16_t",
    "char32_t",
    "class",
    "compl",
    "concept",
    "consteval",
    "constexpr",
    "constinit",
    "const_cast",
    "co_await",
    "co_return",
    "co_yield",
    "decltype",
    "delete",
    "dynamic_cast",
    "explicit",
    "export",
    "false",
    "friend",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "not_eq",
    "nullptr",
    "operator",
    "or",
    "or_eq",
    "private",
    "protected",
    "public",
    "reinterpret_cast",
    "requires",
    "static_assert",
    "static_cast",
    "template",
    "this",
    "thread_local",
    "throw",
    "true",
    "try",
    "typeid",
    "typename",
    "using",
    "virtual",
    "wchar_t",
    "xor",
    "xor_eq",
];
const INDENT: &str = "  ";

pub(super) struct Body {
    pub code: String,
    pub uses_provider: bool,
    /// Standard headers the body needs, e.g. `<vector>`.
    pub includes: BTreeSet<&'static str>,
}

/// One parameter's setup and what is passed for it.
struct Local {
    lines: Vec<String>,
    arg: String,
    include: Option<&'static str>,
}

pub(super) fn body(
    name: &str,
    function: &FunctionMetrics,
    placement: &Placement,
    fills: &[Fill],
) -> Body {
    let params = &function.signature.params;
    // A local named like the function, its class or a namespace would
    // hide it at the call.
    let reserved: Vec<&str> = name
        .split("::")
        .chain(placement.class.iter().flat_map(|c| c.split("::")))
        .collect();
    let vars = variable_names(params, &reserved);
    let last_input = fills.iter().rposition(takes_bytes);
    let locals: Vec<Local> = fills
        .iter()
        .enumerate()
        .map(|(i, fill)| {
            let local = local(fill, i, &vars, &params[i], Some(i) == last_input, name);
            moved_if_rvalue(local, &params[i], &vars[i])
        })
        .collect();
    let is_moving = locals.iter().any(|l| l.arg.starts_with("std::move("));
    // Numbers and hand-built values, then the buffers (the last takes the
    // rest of the input), then lengths set from the buffers' sizes.
    let stage = |fill: &Fill| match fill {
        Fill::Length { .. } => 2,
        fill if takes_bytes(fill) => 1,
        _ => 0,
    };
    let inputs_last = (0..3).flat_map(|s| (0..fills.len()).filter(move |&i| stage(&fills[i]) == s));
    let mut lines: Vec<String> = Vec::new();
    if !fills.is_empty() {
        lines.push("FuzzedDataProvider provider(data, size);".into());
    }
    lines.extend(inputs_last.flat_map(|i| locals[i].lines.clone()));
    let args: Vec<&str> = locals.iter().map(|l| l.arg.as_str()).collect();
    lines.extend(call(name, function, placement, &args));
    lines.push("return 0;".into());
    let indented: Vec<String> = lines.iter().map(|l| format!("{INDENT}{l}")).collect();
    Body {
        code: format!(
            "extern \"C\" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {{\n{}\n}}",
            indented.join("\n")
        ),
        uses_provider: !fills.is_empty(),
        includes: locals
            .iter()
            .filter_map(|l| l.include)
            .chain(is_moving.then_some("<utility>"))
            .collect(),
    }
}

/// An rvalue-reference (`T &&`) parameter takes its local by `std::move`.
fn moved_if_rvalue(local: Local, param: &Param, var: &str) -> Local {
    if param.type_text.contains("&&") && local.arg == var {
        Local {
            arg: format!("std::move({var})"),
            ..local
        }
    } else {
        local
    }
}

/// Each parameter's local variable: its own name when that is usable
/// (an identifier that isn't a keyword, one of the harness's own names,
/// in `reserved`, or a `…_type` alias name), else `arg<position>`.
pub(super) fn variable_names(params: &[Param], reserved: &[&str]) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    for (i, param) in params.iter().enumerate() {
        let own = param.name.as_deref().filter(|n| {
            is_identifier(n)
                && !RESERVED.contains(n)
                && !reserved.contains(n)
                && !CPP_KEYWORDS.contains(n)
                && !n.ends_with("_type")
                && !used.iter().any(|u| u == n)
        });
        let mut name = own.map_or_else(|| format!("arg{}", i + 1), str::to_string);
        while used.contains(&name) {
            name.push('_');
        }
        used.push(name);
    }
    used
}

fn takes_bytes(fill: &Fill) -> bool {
    fill.is_input()
}

fn local(
    fill: &Fill,
    i: usize,
    vars: &[String],
    param: &Param,
    is_last: bool,
    function: &str,
) -> Local {
    let var = &vars[i];
    let is_path = param.name.as_deref().is_some_and(is_path_name);
    let text = |arg: String| text_local(var, arg, is_path, is_last);
    match fill {
        Fill::Bytes {
            element,
            is_const,
            length,
        } => {
            let leaf = comment_safe(split_name(function).1);
            let note = length.is_none().then(|| {
                format!("// {TODO} {var} has no length parameter: check how many bytes {leaf} reads from it.")
            });
            vector_local(
                var,
                "uint8_t",
                is_last,
                note,
                bytes_arg(var, element, *is_const),
            )
        }
        Fill::Length { of, pointee: None } => Local {
            lines: Vec::new(),
            arg: format!("{}.size()", vars[*of]),
            include: None,
        },
        Fill::Length {
            of,
            pointee: Some(ty),
        } => {
            if !is_safe_type(ty) {
                return unreadable(i);
            }
            Local {
                lines: vec![format!("{ty} {var} = {}.size();", vars[*of])],
                arg: format!("&{var}"),
                include: None,
            }
        }
        Fill::CString { is_const: true } => text(format!("{var}.c_str()")),
        Fill::CString { is_const: false } => text(format!("{var}.data()")),
        Fill::Text => text(var.clone()),
        Fill::ByteVector { element } => vector_local(var, element, is_last, None, var.clone()),
        Fill::Number {
            ty,
            kind,
            by_address,
        } => number(var, ty, *kind, *by_address, i),
        Fill::Manual => manual(var, &param.type_text, i),
    }
}

/// A `std::string` of input, passed as `arg`.
fn text_local(var: &str, arg: String, is_path: bool, is_last: bool) -> Local {
    let take = if is_last {
        "provider.ConsumeRemainingBytesAsString()"
    } else {
        "provider.ConsumeRandomLengthString()"
    };
    let note = is_path.then(|| {
        format!("// {TODO} {var} is a file name: write the input to a temporary file and pass its path.")
    });
    Local {
        lines: note
            .into_iter()
            .chain([format!("std::string {var} = {take};")])
            .collect(),
        arg,
        include: Some("<string>"),
    }
}

/// A `std::vector` of input bytes, passed as `arg`.
fn vector_local(
    var: &str,
    element: &str,
    is_last: bool,
    note: Option<String>,
    arg: String,
) -> Local {
    Local {
        lines: note
            .into_iter()
            .chain([format!(
                "std::vector<{element}> {var} = {};",
                bytes_take(element, is_last)
            )])
            .collect(),
        arg,
        include: Some("<vector>"),
    }
}

fn bytes_take(element: &str, is_last: bool) -> String {
    if is_last {
        format!("provider.ConsumeRemainingBytes<{element}>()")
    } else {
        format!(
            "provider.ConsumeBytes<{element}>(provider.ConsumeIntegralInRange<size_t>(0, provider.remaining_bytes()))"
        )
    }
}

/// The vector's bytes as the pointer type the parameter wants.
fn bytes_arg(var: &str, element: &str, is_const: bool) -> String {
    if matches!(element, "uint8_t" | "std::uint8_t" | "void") || !is_safe_type(element) {
        return format!("{var}.data()");
    }
    let qualifier = if is_const { "const " } else { "" };
    format!("reinterpret_cast<{qualifier}{element} *>({var}.data())")
}

fn number(var: &str, ty: &str, kind: NumberKind, by_address: bool, i: usize) -> Local {
    if !is_safe_type(ty) {
        return unreadable(i);
    }
    let line = match kind {
        NumberKind::Integer => format!("{ty} {var} = provider.ConsumeIntegral<{ty}>();"),
        NumberKind::Float => format!("{ty} {var} = provider.ConsumeFloatingPoint<{ty}>();"),
        NumberKind::Bool => format!("bool {var} = provider.ConsumeBool();"),
    };
    Local {
        lines: vec![line],
        arg: if by_address {
            format!("&{var}")
        } else {
            var.to_string()
        },
        include: None,
    }
}

/// A value the user has to set up: declared empty, with a TODO.
fn manual(var: &str, type_text: &str, i: usize) -> Local {
    if !is_safe_type(type_text) {
        return unreadable(i);
    }
    let is_reference = type_text.contains('&');
    let declared = if is_reference {
        format!("std::remove_cv_t<std::remove_reference_t<{var}_type>>")
    } else {
        format!("{var}_type")
    };
    Local {
        lines: vec![
            format!("using {var}_type = {type_text};"),
            format!(
                "{declared} {var}{{}};  // {TODO} set up {var} ({type_text}); it starts out empty"
            ),
        ],
        arg: var.to_string(),
        include: is_reference.then_some("<type_traits>"),
    }
}

fn unreadable(i: usize) -> Local {
    Local {
        lines: vec![format!(
            "// {TODO} parameter {}'s type couldn't be read: pass a real value.",
            i + 1
        )],
        arg: "{}".into(),
        include: None,
    }
}

/// The call itself, with any object it needs.
fn call(
    name: &str,
    function: &FunctionMetrics,
    placement: &Placement,
    args: &[&str],
) -> Vec<String> {
    let leaf = split_name(name).1;
    let shown = comment_safe(leaf);
    let args = args.join(", ");
    let mut lines = Vec::new();
    if function.signature.params.is_empty() {
        lines.push(format!("// {TODO} {shown} takes no parameters: find where its input comes from (globals, files, the environment)."));
    }
    if function.signature.is_variadic {
        lines.push(format!(
            "// {shown} takes variable arguments: only the fixed ones are passed."
        ));
    }
    if function.signature.is_template {
        lines.push(format!(
            "// {TODO} {shown} is a template: name its template arguments in the call."
        ));
    }
    let class = placement
        .class
        .as_deref()
        .filter(|c| is_qualified_identifier(c));
    match class {
        _ if !is_qualified_identifier(name) => lines.push(format!(
            "// {TODO} call the function here (its name isn't a plain identifier)."
        )),
        Some(class) if placement.is_constructor => {
            lines.push(if args.is_empty() {
                format!("{class} object{{}};")
            } else {
                format!("{class} object({args});")
            });
            lines.push("(void)object;".into());
        }
        Some(class) if placement.needs_object => {
            lines.push(format!(
                "{class} object{{}};  // {TODO} construct a real {class}"
            ));
            lines.push(format!("object.{leaf}({args});"));
        }
        _ => lines.push(format!("{name}({args});")),
    }
    lines
}
