//! Built-in demo project shown before any real project is opened. It runs
//! through the real indexer, so it doubles as an end-to-end smoke test.

use crate::index::{build_graph, Extractor, SourceFile};
use crate::model::CodeGraph;

const DEMO_SOURCES: &[(&str, &str)] = &[
    (
        "shape.h",
        "class Shape {\n\
         public:\n\
         \x20   virtual ~Shape() = default;\n\
         \x20   virtual double area() const = 0;\n\
         };\n",
    ),
    (
        "circle.h",
        "#include \"shape.h\"\n\
         \n\
         class Circle : public Shape {\n\
         public:\n\
         \x20   explicit Circle(double r) : radius(r) {}\n\
         \x20   double area() const override {\n\
         \x20       return 3.14159265 * radius * radius;\n\
         \x20   }\n\
         private:\n\
         \x20   double radius;\n\
         };\n",
    ),
    (
        "square.h",
        "#include \"shape.h\"\n\
         \n\
         class Square : public Shape {\n\
         public:\n\
         \x20   explicit Square(double s) : side(s) {}\n\
         \x20   double area() const override {\n\
         \x20       return side * side;\n\
         \x20   }\n\
         private:\n\
         \x20   double side;\n\
         };\n",
    ),
    (
        "main.cpp",
        "#include \"circle.h\"\n\
         #include \"square.h\"\n\
         \n\
         double total_area(const Circle& c, const Square& s) {\n\
         \x20   return c.area() + s.area();\n\
         }\n\
         \n\
         int main() {\n\
         \x20   Circle c(2.0);\n\
         \x20   Square s(3.0);\n\
         \x20   return static_cast<int>(total_area(c, s));\n\
         }\n",
    ),
];

/// Indexes the embedded demo sources. Fails only if the C++ grammar can't
/// be loaded.
pub fn demo_codebase() -> Result<CodeGraph, String> {
    let mut extractor = Extractor::new()?;
    let files = DEMO_SOURCES
        .iter()
        .map(|(path, contents)| {
            let facts = extractor
                .extract(contents)
                .ok_or_else(|| format!("failed to parse demo file {path}"))?;
            Ok(SourceFile {
                path: path.to_string(),
                contents: contents.to_string(),
                facts,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(build_graph(files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EdgeKind;

    #[test]
    fn demo_indexes_into_expected_graph() {
        let graph = demo_codebase().expect("demo");
        let id = |q: &str| {
            graph
                .symbols
                .values()
                .find(|s| s.qualified_name == q)
                .map(|s| s.id)
                .unwrap_or_else(|| panic!("missing {q}"))
        };
        let has = |from: &str, to: &str, kind: EdgeKind| {
            let (f, t) = (id(from), id(to));
            graph
                .edges
                .iter()
                .any(|e| e.from == f && e.to == t && e.kind == kind)
        };
        assert!(has("Circle", "Shape", EdgeKind::Inherits));
        assert!(has("Square", "Shape", EdgeKind::Inherits));
        assert!(has("total_area", "Circle::area", EdgeKind::Calls));
        assert!(has("total_area", "Square::area", EdgeKind::Calls));
        assert!(has("main", "total_area", EdgeKind::Calls));
        assert!(has("Circle::area", "Circle::radius", EdgeKind::Uses));
        assert_eq!(graph.file_contents.len(), DEMO_SOURCES.len());
    }
}
