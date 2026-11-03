//! Dev tool: print the tree-sitter C++ syntax tree of a file.
//! Usage: cargo run --example dump_tree -- path/to/file.cpp
fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_tree <file>");
    let source = std::fs::read_to_string(&path).expect("read file");
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .expect("load C++ grammar");
    let tree = parser.parse(&source, None).expect("parse");
    print_node(tree.root_node(), &source, 0, None);
}

fn print_node(node: tree_sitter::Node, src: &str, depth: usize, field: Option<&str>) {
    let text = if node.child_count() == 0 {
        format!(" {:?}", &src[node.byte_range()])
    } else {
        String::new()
    };
    let field = field.map(|f| format!("{f}: ")).unwrap_or_default();
    if node.is_named() {
        println!("{}{}{}{}", "  ".repeat(depth), field, node.kind(), text);
    }
    let mut cursor = node.walk();
    for (i, child) in node.children(&mut cursor).enumerate() {
        print_node(child, src, depth + 1, node.field_name_for_child(i as u32));
    }
}
