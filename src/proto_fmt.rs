use anyhow::Result;
use tree_sitter::Node;

use crate::{proto_sort::SortKey, syntax::parse_proto};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub sort_imports: bool,
    pub sort_fields: bool,
    pub sort_declarations: bool,
    pub field_key: SortKey,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            sort_imports: true,
            sort_fields: true,
            sort_declarations: false,
            field_key: SortKey::Number,
        }
    }
}

pub fn format_proto(source: &str, options: FormatOptions) -> Result<String> {
    let tree = parse_proto(source)?;
    if tree.root_node().has_error() {
        anyhow::bail!("cannot format protobuf source with syntax errors");
    }

    let mut renderer = Renderer { source, options };
    let mut formatted = renderer.render_source(tree.root_node());
    formatted.push('\n');

    let verify = parse_proto(&formatted)?;
    if verify.root_node().has_error() {
        anyhow::bail!("formatter produced invalid protobuf syntax");
    }
    Ok(formatted)
}

struct Renderer<'a> {
    source: &'a str,
    options: FormatOptions,
}

#[derive(Debug, Clone)]
struct Item<'a> {
    node: Node<'a>,
    leading_comments: Vec<Node<'a>>,
}

impl<'a> Renderer<'a> {
    fn render_source(&mut self, node: Node<'a>) -> String {
        let mut items = self.collect_items(node);
        if self.options.sort_imports {
            sort_imports(&mut items, self.source);
        }
        if self.options.sort_declarations {
            sort_declarations(&mut items, self.source);
        }
        self.render_items(&items, 0)
    }

    fn render_items(&mut self, items: &[Item<'a>], indent: usize) -> String {
        let mut out = String::new();
        let mut previous_group: Option<Group> = None;

        for item in items {
            let group = group_for_kind(item.node.kind());
            if !out.is_empty() && should_separate(previous_group, group, indent) {
                out.push('\n');
            }

            for comment in &item.leading_comments {
                out.push_str(&indent_text(indent));
                out.push_str(self.node_text(*comment).trim());
                out.push('\n');
            }

            let rendered = self.render_node(item.node, indent);
            if !rendered.is_empty() {
                out.push_str(&rendered);
                out.push('\n');
            }
            previous_group = Some(group);
        }

        while out.ends_with('\n') {
            out.pop();
        }
        out
    }

    fn render_node(&mut self, node: Node<'a>, indent: usize) -> String {
        match node.kind() {
            "syntax" | "edition" | "package" | "import" | "option" | "reserved" | "extensions"
            | "field" | "map_field" | "oneof_field" | "enum_field" | "rpc" | "empty_statement" => {
                format!(
                    "{}{}",
                    indent_text(indent),
                    normalize_statement(self.node_text(node))
                )
            }
            "message" => {
                self.render_compound(node, "message", "message_name", "message_body", indent)
            }
            "enum" => self.render_compound(node, "enum", "enum_name", "enum_body", indent),
            "service" => self.render_compound(node, "service", "service_name", None, indent),
            "oneof" => self.render_oneof(node, indent),
            "extend" => self.render_extend(node, indent),
            "comment" => format!("{}{}", indent_text(indent), self.node_text(node).trim()),
            _ => self.render_fallback(node, indent),
        }
    }

    fn render_compound(
        &mut self,
        node: Node<'a>,
        keyword: &str,
        name_kind: &str,
        body_kind: impl Into<Option<&'static str>>,
        indent: usize,
    ) -> String {
        let body_kind = body_kind.into();
        let name = child_text(node, name_kind, self.source).unwrap_or_default();
        let body = body_kind.and_then(|kind| child_by_kind(node, kind));
        let mut out = format!("{}{} {} {{", indent_text(indent), keyword, name.trim());
        if let Some(body) = body {
            let mut items = self.collect_items(body);
            if self.options.sort_declarations {
                sort_declarations(&mut items, self.source);
            }
            if self.options.sort_fields {
                sort_fields(&mut items, self.source, self.options.field_key);
            }
            if !items.is_empty() {
                out.push('\n');
                out.push_str(&self.render_items(&items, indent + 1));
                out.push('\n');
                out.push_str(&indent_text(indent));
            }
        } else {
            let items = self
                .named_children(node)
                .into_iter()
                .filter(|child| child.kind() != name_kind)
                .collect::<Vec<_>>();
            if !items.is_empty() {
                out.push('\n');
                for child in items {
                    out.push_str(&self.render_node(child, indent + 1));
                    out.push('\n');
                }
                out.push_str(&indent_text(indent));
            }
        }
        out.push('}');
        out
    }

    fn render_oneof(&mut self, node: Node<'a>, indent: usize) -> String {
        let name = self
            .named_children(node)
            .into_iter()
            .find(|child| child.kind() == "identifier")
            .map(|child| self.node_text(child).trim().to_owned())
            .unwrap_or_default();

        let mut items = self
            .collect_items(node)
            .into_iter()
            .filter(|item| item.node.kind() != "identifier")
            .collect::<Vec<_>>();
        if self.options.sort_fields {
            sort_fields(&mut items, self.source, self.options.field_key);
        }

        let mut out = format!("{}oneof {} {{", indent_text(indent), name);
        if !items.is_empty() {
            out.push('\n');
            out.push_str(&self.render_items(&items, indent + 1));
            out.push('\n');
            out.push_str(&indent_text(indent));
        }
        out.push('}');
        out
    }

    fn render_extend(&mut self, node: Node<'a>, indent: usize) -> String {
        let name = self
            .named_children(node)
            .into_iter()
            .find(|child| child.kind() == "full_ident")
            .map(|child| self.node_text(child).trim().to_owned())
            .unwrap_or_default();
        let body = child_by_kind(node, "message_body");
        let mut out = format!("{}extend {} {{", indent_text(indent), name);
        if let Some(body) = body {
            let mut items = self.collect_items(body);
            if self.options.sort_fields {
                sort_fields(&mut items, self.source, self.options.field_key);
            }
            if !items.is_empty() {
                out.push('\n');
                out.push_str(&self.render_items(&items, indent + 1));
                out.push('\n');
                out.push_str(&indent_text(indent));
            }
        }
        out.push('}');
        out
    }

    fn render_fallback(&self, node: Node<'a>, indent: usize) -> String {
        self.node_text(node)
            .lines()
            .map(|line| format!("{}{}", indent_text(indent), line.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn collect_items(&self, node: Node<'a>) -> Vec<Item<'a>> {
        let mut items = Vec::new();
        let mut comments = Vec::new();
        for child in self.named_children(node) {
            match child.kind() {
                "comment" => comments.push(child),
                "empty_statement" => {}
                kind if is_name_or_body_kind(kind) => {}
                _ => {
                    items.push(Item {
                        node: child,
                        leading_comments: std::mem::take(&mut comments),
                    });
                }
            }
        }
        for comment in comments {
            items.push(Item {
                node: comment,
                leading_comments: Vec::new(),
            });
        }
        items
    }

    fn named_children(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .filter(|child| child.is_named())
            .collect()
    }

    fn node_text(&self, node: Node<'a>) -> &'a str {
        node.utf8_text(self.source.as_bytes()).unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Header,
    Import,
    Option,
    Declaration,
    Member,
    Comment,
}

fn group_for_kind(kind: &str) -> Group {
    match kind {
        "syntax" | "edition" | "package" => Group::Header,
        "import" => Group::Import,
        "option" => Group::Option,
        "message" | "enum" | "service" | "extend" => Group::Declaration,
        "comment" => Group::Comment,
        _ => Group::Member,
    }
}

fn should_separate(previous: Option<Group>, current: Group, indent: usize) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if indent > 0 {
        return matches!(current, Group::Declaration) || matches!(previous, Group::Declaration);
    }
    previous != current
        || matches!(current, Group::Declaration)
        || matches!(current, Group::Option)
        || matches!(current, Group::Comment)
}

fn sort_imports(items: &mut [Item<'_>], source: &str) {
    let mut imports = items
        .iter()
        .filter(|item| item.node.kind() == "import")
        .cloned()
        .collect::<Vec<_>>();
    if imports.len() < 2 {
        return;
    }
    imports.sort_by(|a, b| node_text(a.node, source).cmp(node_text(b.node, source)));
    let mut imports = imports.into_iter();
    for item in items.iter_mut().filter(|item| item.node.kind() == "import") {
        *item = imports.next().unwrap();
    }
}

fn sort_declarations(items: &mut [Item<'_>], source: &str) {
    let mut declarations = items
        .iter()
        .filter(|item| is_declaration_kind(item.node.kind()))
        .cloned()
        .collect::<Vec<_>>();
    if declarations.len() < 2 {
        return;
    }
    declarations
        .sort_by(|a, b| declaration_key(a.node, source).cmp(&declaration_key(b.node, source)));
    let mut declarations = declarations.into_iter();
    for item in items
        .iter_mut()
        .filter(|item| is_declaration_kind(item.node.kind()))
    {
        *item = declarations.next().unwrap();
    }
}

fn sort_fields(items: &mut [Item<'_>], source: &str, key: SortKey) {
    let mut fields = items
        .iter()
        .filter(|item| is_field_kind(item.node.kind()))
        .cloned()
        .collect::<Vec<_>>();
    if fields.len() < 2 {
        return;
    }
    fields.sort_by(|a, b| field_key(a.node, source, key).cmp(&field_key(b.node, source, key)));
    let mut fields = fields.into_iter();
    for item in items
        .iter_mut()
        .filter(|item| is_field_kind(item.node.kind()))
    {
        *item = fields.next().unwrap();
    }
}

fn declaration_key(node: Node<'_>, source: &str) -> String {
    let name_kind = match node.kind() {
        "message" => "message_name",
        "enum" => "enum_name",
        "service" => "service_name",
        "extend" => "full_ident",
        _ => "",
    };
    let name = child_text(node, name_kind, source).unwrap_or_default();
    format!("{}:{name}", node.kind())
}

fn field_key(node: Node<'_>, source: &str, key: SortKey) -> String {
    match key {
        SortKey::Name => field_name(node, source),
        SortKey::Number => format!(
            "{:010}:{}",
            field_number(node, source).unwrap_or(u32::MAX),
            field_name(node, source)
        ),
    }
}

fn field_name(node: Node<'_>, source: &str) -> String {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.is_named() && child.kind() == "identifier")
        .last()
        .map(|child| node_text(child, source).trim().to_owned())
        .unwrap_or_default()
}

fn field_number(node: Node<'_>, source: &str) -> Option<u32> {
    child_by_kind(node, "field_number")
        .or_else(|| child_by_kind(node, "int_lit"))
        .and_then(|child| {
            node_text(child, source)
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
}

fn child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == kind)
}

fn child_text<'a>(node: Node<'a>, kind: &str, source: &'a str) -> Option<&'a str> {
    child_by_kind(node, kind).map(|child| node_text(child, source))
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn is_name_or_body_kind(kind: &str) -> bool {
    matches!(
        kind,
        "message_name" | "enum_name" | "service_name" | "message_body" | "enum_body"
    )
}

fn is_declaration_kind(kind: &str) -> bool {
    matches!(kind, "message" | "enum" | "service" | "extend")
}

fn is_field_kind(kind: &str) -> bool {
    matches!(kind, "field" | "map_field" | "oneof_field" | "enum_field")
}

fn normalize_statement(text: &str) -> String {
    let mut normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (from, to) in [
        (" =", "="),
        ("= ", "="),
        ("=", " = "),
        (" ;", ";"),
        (" ,", ","),
        (",", ", "),
        ("  ", " "),
        (" (", "("),
        ("( ", "("),
        (" )", ")"),
        ("[ ", "["),
        (" ]", "]"),
        (" <", "<"),
        ("< ", "<"),
        (" >", ">"),
    ] {
        normalized = normalized.replace(from, to);
    }
    while normalized.contains("  ") {
        normalized = normalized.replace("  ", " ");
    }
    normalized.trim().to_owned()
}

fn indent_text(indent: usize) -> String {
    "  ".repeat(indent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_sorting_imports_and_fields() {
        let input = r#"syntax = "proto3";
import "z.proto";
import "a.proto";
message Z { string b = 2; string a = 1; }
message A {}
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(
            output.find("import \"a.proto\"").unwrap() < output.find("import \"z.proto\"").unwrap()
        );
        assert!(output.find("message Z").unwrap() < output.find("message A").unwrap());
        assert!(output.contains("message Z {\n  string a = 1;\n  string b = 2;\n}"));
    }

    #[test]
    fn can_sort_declarations_when_requested() {
        let input = r#"syntax = "proto3";
message Z {}
message A {}
"#;
        let output = format_proto(
            input,
            FormatOptions {
                sort_declarations: true,
                ..FormatOptions::default()
            },
        )
        .unwrap();
        assert!(output.find("message A").unwrap() < output.find("message Z").unwrap());
    }

    #[test]
    fn keeps_map_type_separated_from_field_name() {
        let input = r#"syntax = "proto3";
message Labels { map<string, string> labels=1; }
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(output.contains("map<string, string> labels = 1;"));
    }

    #[test]
    fn formats_nested_blocks_from_cst() {
        let input = r#"syntax = "proto3";
message Outer { message Z {} message A {} enum E { TWO = 2; ONE = 1; } string b=2; string a=1; }
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(output.contains("message A {}") || output.contains("message A {\n}"));
        assert!(output.find("message Z").unwrap() < output.find("message A").unwrap());
        assert!(output.find("string a = 1").unwrap() < output.find("string b = 2").unwrap());
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(format_proto("message Foo { string name = ; }", FormatOptions::default()).is_err());
    }
}
