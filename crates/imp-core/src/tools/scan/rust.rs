//! Rust tree-sitter extraction — structs, enums, traits, impls, functions.
//! Ported from uu-manifest's Rust language adapter.

use tree_sitter::{Node, Parser};

use super::types::*;

pub fn parse(source: &str, file: &str, result: &mut ScanResult) {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        return;
    }
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return,
    };
    extract_rust(&tree.root_node(), source, file, result);
}

fn extract_rust(root: &Node, source: &str, file: &str, result: &mut ScanResult) {
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "struct_item" => extract_struct(&child, source, file, result),
            "enum_item" => extract_enum(&child, source, file, result),
            "trait_item" => extract_trait(&child, source, file, result),
            "impl_item" => extract_impl(&child, source, file, result),
            "function_item" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|node| node_text(&node, source).to_string());
                extract_function(&child, source, file, result);
                if let Some(name) = name {
                    collect_call_edges(&child, source, file, &name, result);
                }
            }
            "use_declaration" => extract_use(&child, source, file, result),
            _ => {}
        }
    }
}

fn extract_struct(node: &Node, source: &str, file: &str, result: &mut ScanResult) {
    let name = match get_name(node, source) {
        Some(n) => n,
        None => return,
    };
    let vis = get_visibility(node, source);
    let fields = extract_fields(node, source);

    result.types.insert(
        name.clone(),
        TypeInfo {
            name,
            source: source_loc(file, node),
            kind: TypeKind::Struct,
            fields,
            visibility: vis,
            ..Default::default()
        },
    );
}

fn extract_fields(node: &Node, source: &str) -> Vec<Field> {
    let mut fields = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() == "field_declaration" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, source).to_string();
                    let type_name = child
                        .child_by_field_name("type")
                        .map(|t| node_text(&t, source).to_string())
                        .unwrap_or_default();
                    let optional = type_name.starts_with("Option<");
                    fields.push(Field {
                        name,
                        type_name,
                        optional,
                    });
                }
            }
        }
    }
    fields
}

fn extract_enum(node: &Node, source: &str, file: &str, result: &mut ScanResult) {
    let name = match get_name(node, source) {
        Some(n) => n,
        None => return,
    };
    let vis = get_visibility(node, source);

    let mut variants = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() == "enum_variant" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    variants.push(node_text(&name_node, source).to_string());
                }
            }
        }
    }

    result.types.insert(
        name.clone(),
        TypeInfo {
            name,
            source: source_loc(file, node),
            kind: TypeKind::Enum,
            variants,
            visibility: vis,
            ..Default::default()
        },
    );
}

fn extract_trait(node: &Node, source: &str, file: &str, result: &mut ScanResult) {
    let name = match get_name(node, source) {
        Some(n) => n,
        None => return,
    };
    let vis = get_visibility(node, source);

    let mut methods = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() == "function_signature_item" || child.kind() == "function_item" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    methods.push(node_text(&name_node, source).to_string());
                }
            }
        }
    }

    result.types.insert(
        name.clone(),
        TypeInfo {
            name,
            source: source_loc(file, node),
            kind: TypeKind::Trait,
            methods,
            visibility: vis,
            ..Default::default()
        },
    );
}

fn extract_impl(node: &Node, source: &str, file: &str, result: &mut ScanResult) {
    let type_node = match node.child_by_field_name("type") {
        Some(n) => n,
        None => return,
    };
    let type_name = node_text(&type_node, source).to_string();

    let trait_name = node
        .child_by_field_name("trait")
        .map(|t| node_text(&t, source).to_string());
    if let Some(trait_name) = &trait_name {
        result.edges.push(EdgeInfo {
            from: type_name.clone(),
            to: trait_name.clone(),
            source: source_loc(file, node),
            kind: EdgeKind::Implements,
            label: Some(format!("{type_name} implements {trait_name}")),
        });
    }

    let mut methods = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() == "function_item" {
                let vis = get_visibility(&child, source);
                if let Some(name_node) = child.child_by_field_name("name") {
                    let method_name = node_text(&name_node, source).to_string();
                    methods.push(method_name.clone());
                    result.edges.push(EdgeInfo {
                        from: type_name.clone(),
                        to: method_name.clone(),
                        source: source_loc(file, &child),
                        kind: EdgeKind::Contains,
                        label: Some(format!("{type_name} contains {method_name}")),
                    });
                    collect_call_edges(&child, source, file, &method_name, result);

                    if matches!(vis, Visibility::Public) {
                        let sig = build_fn_signature(&child, source);
                        let is_async = has_async(&child, source);
                        let is_test = has_test_attr(&child, source);
                        let qualified = format!("{}::{}", type_name, method_name);
                        result.functions.insert(
                            qualified,
                            FunctionInfo {
                                name: method_name,
                                source: source_loc(file, &child),
                                signature: sig,
                                visibility: vis,
                                is_async,
                                is_test,
                            },
                        );
                    }
                }
            }
        }
    }

    if let Some(typedef) = result.types.get_mut(&type_name) {
        for m in &methods {
            if !typedef.methods.contains(m) {
                typedef.methods.push(m.clone());
            }
        }
        if let Some(trait_name) = &trait_name {
            if !typedef.implements.contains(trait_name) {
                typedef.implements.push(trait_name.clone());
            }
        }
    }
}

fn extract_function(node: &Node, source: &str, file: &str, result: &mut ScanResult) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(&n, source).to_string(),
        None => return,
    };
    let vis = get_visibility(node, source);
    let sig = build_fn_signature(node, source);
    let is_async = has_async(node, source);
    let is_test = has_test_attr(node, source);

    result.functions.insert(
        name.clone(),
        FunctionInfo {
            name,
            source: source_loc(file, node),
            signature: sig,
            visibility: vis,
            is_async,
            is_test,
        },
    );
}

fn extract_use(node: &Node, source: &str, file: &str, result: &mut ScanResult) {
    let text = node_text(node, source)
        .trim()
        .trim_start_matches("use")
        .trim()
        .trim_end_matches(';')
        .trim();
    if text.is_empty() {
        return;
    }
    result.edges.push(EdgeInfo {
        from: file.to_string(),
        to: text.to_string(),
        source: source_loc(file, node),
        kind: EdgeKind::Imports,
        label: Some(format!("imports {text}")),
    });
}

fn collect_call_edges(
    node: &Node,
    source: &str,
    file: &str,
    caller: &str,
    result: &mut ScanResult,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "call_expression" => {
                if let Some(callee) = extract_call_target(&child, source) {
                    result.edges.push(EdgeInfo {
                        from: caller.to_string(),
                        to: callee.clone(),
                        source: source_loc(file, &child),
                        kind: EdgeKind::Calls,
                        label: Some(format!("calls {callee}")),
                    });
                }
            }
            "method_call_expression" => {
                if let Some(callee) = child
                    .child_by_field_name("name")
                    .or_else(|| child.child_by_field_name("field"))
                    .map(|node| node_text(&node, source).to_string())
                    .or_else(|| first_named_child_text(&child, source, "field_identifier"))
                {
                    result.edges.push(EdgeInfo {
                        from: caller.to_string(),
                        to: callee.clone(),
                        source: source_loc(file, &child),
                        kind: EdgeKind::Calls,
                        label: Some(format!("method call {callee}")),
                    });
                }
            }
            _ => collect_call_edges(&child, source, file, caller, result),
        }
    }
}

fn extract_call_target(node: &Node, source: &str) -> Option<String> {
    let function = node.child_by_field_name("function")?;
    match function.kind() {
        "identifier" | "scoped_identifier" => Some(node_text(&function, source).to_string()),
        "field_expression" => function
            .child_by_field_name("field")
            .map(|field| node_text(&field, source).to_string()),
        _ => first_named_child_text(&function, source, "identifier")
            .or_else(|| first_named_child_text(&function, source, "scoped_identifier"))
            .or_else(|| first_named_child_text(&function, source, "field_identifier")),
    }
}

fn first_named_child_text(node: &Node, source: &str, kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

// ── helpers ─────────────────────────────────────────────────────────

fn get_visibility(node: &Node, source: &str) -> Visibility {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let text = node_text(&child, source);
            return if text.contains("pub(crate)") || text.contains("pub(super)") {
                Visibility::Internal
            } else {
                Visibility::Public
            };
        }
    }
    Visibility::Private
}

fn node_text<'a>(node: &Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn get_name(node: &Node, source: &str) -> Option<String> {
    for field_name in &["name", "type"] {
        if let Some(name_node) = node.child_by_field_name(field_name) {
            return Some(node_text(&name_node, source).to_string());
        }
    }
    None
}

fn source_loc(file: &str, node: &Node) -> String {
    format!("{}:{}", file, node.start_position().row + 1)
}

fn build_fn_signature(node: &Node, source: &str) -> String {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, source))
        .unwrap_or("?");
    let params = node
        .child_by_field_name("parameters")
        .map(|n| node_text(&n, source))
        .unwrap_or("()");
    let ret = node
        .child_by_field_name("return_type")
        .map(|n| format!(" -> {}", node_text(&n, source)))
        .unwrap_or_default();
    let async_prefix = if has_async(node, source) {
        "async "
    } else {
        ""
    };
    format!("{async_prefix}fn {name}{params}{ret}")
}

fn has_async(node: &Node, source: &str) -> bool {
    let text = node_text(node, source);
    text.starts_with("async ") || text.starts_with("pub async ") || text.contains(" async fn ")
}

fn has_test_attr(node: &Node, source: &str) -> bool {
    if let Some(parent) = node.parent() {
        let idx = node.start_byte();
        let mut cursor = parent.walk();
        for child in parent.named_children(&mut cursor) {
            if child.start_byte() >= idx {
                break;
            }
            if child.kind() == "attribute_item" || child.kind() == "inner_attribute_item" {
                let text = node_text(&child, source);
                if text.contains("test") {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_rust_str(source: &str) -> ScanResult {
        let mut result = ScanResult::default();
        parse(source, "src/lib.rs", &mut result);
        result
    }

    #[test]
    fn pub_struct_with_fields() {
        let r = parse_rust_str(
            r#"
pub struct User {
    pub name: String,
    pub age: u32,
    pub email: Option<String>,
}
"#,
        );
        let t = &r.types["User"];
        assert_eq!(t.kind, TypeKind::Struct);
        assert_eq!(t.visibility, Visibility::Public);
        assert_eq!(t.fields.len(), 3);
        assert_eq!(t.fields[0].name, "name");
        assert!(!t.fields[0].optional);
        assert!(t.fields[2].optional);
    }

    #[test]
    fn private_struct() {
        let r = parse_rust_str("struct Internal { x: i32 }");
        assert_eq!(r.types["Internal"].visibility, Visibility::Private);
    }

    #[test]
    fn enum_variants() {
        let r = parse_rust_str("pub enum Color { Red, Green, Blue }");
        let t = &r.types["Color"];
        assert_eq!(t.kind, TypeKind::Enum);
        assert_eq!(t.variants, vec!["Red", "Green", "Blue"]);
    }

    #[test]
    fn trait_methods() {
        let r = parse_rust_str(
            r#"
pub trait Drawable {
    fn draw(&self);
    fn resize(&mut self, w: u32, h: u32);
}
"#,
        );
        let t = &r.types["Drawable"];
        assert_eq!(t.kind, TypeKind::Trait);
        assert!(t.methods.contains(&"draw".to_string()));
        assert!(t.methods.contains(&"resize".to_string()));
    }

    #[test]
    fn function_with_signature() {
        let r = parse_rust_str("pub fn process(input: &str) -> Result<String> { todo!() }");
        let f = &r.functions["process"];
        assert_eq!(f.visibility, Visibility::Public);
        assert!(f.signature.contains("-> Result<String>"));
    }

    #[test]
    fn async_function() {
        let r = parse_rust_str("pub async fn fetch(url: &str) -> Vec<u8> { todo!() }");
        let f = &r.functions["fetch"];
        assert!(f.is_async);
        assert!(f.signature.starts_with("async fn"));
    }

    #[test]
    fn impl_adds_methods_and_traits() {
        let r = parse_rust_str(
            r#"
pub struct Foo { val: i32 }
impl Foo {
    pub fn new(val: i32) -> Self { Self { val } }
    fn internal(&self) {}
}
impl Display for Foo {
    fn fmt(&self, f: &mut Formatter) -> Result { todo!() }
}
"#,
        );
        let t = &r.types["Foo"];
        assert!(t.methods.contains(&"new".to_string()));
        assert!(t.methods.contains(&"internal".to_string()));
        assert!(t.implements.contains(&"Display".to_string()));
        assert!(r.functions.contains_key("Foo::new"));
    }

    #[test]
    fn rust_scan_edge_extracts_imports_impls_and_calls() {
        let r = parse_rust_str(
            r#"
use crate::helpers::make_widget;

pub trait Render { fn render(&self); }
pub struct Widget;

impl Render for Widget {
    pub fn render(&self) {
        make_widget();
        crate::helpers::save_widget();
        self.finish();
    }

    pub fn finish(&self) {}
}
"#,
        );

        assert!(r.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Imports && edge.to == "crate::helpers::make_widget"
        }));
        assert!(r.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Implements && edge.from == "Widget" && edge.to == "Render"
        }));
        assert!(r.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Contains && edge.from == "Widget" && edge.to == "render"
        }));
        assert!(r.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Calls && edge.from == "render" && edge.to == "make_widget"
        }));
        assert!(r.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Calls
                && edge.from == "render"
                && edge.to == "crate::helpers::save_widget"
        }));
        assert!(r.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Calls && edge.from == "render" && edge.to == "finish"
        }));
    }

    #[test]
    fn source_location() {
        let r = parse_rust_str("\npub struct Pos;");
        assert_eq!(r.types["Pos"].source, "src/lib.rs:2");
    }
}
