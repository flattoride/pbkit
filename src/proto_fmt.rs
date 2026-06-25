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
            sort_declarations: true,
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
    trailing_comments: Vec<Node<'a>>,
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

            let mut rendered = self.render_node(item.node, indent);
            if !rendered.is_empty() {
                append_trailing_comments(&mut rendered, &item.trailing_comments, self.source);
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
            "option" => self.render_option(node, indent),
            "rpc" => self.render_rpc(node, indent),
            "field" | "map_field" | "oneof_field" | "enum_field" => self.render_field(node, indent),
            "reserved" | "extensions" => {
                format!(
                    "{}{}",
                    indent_text(indent),
                    normalize_reserved_or_extensions(self.node_text(node))
                )
            }
            "syntax" | "edition" | "package" | "import" | "empty_statement" => {
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

    fn render_field(&self, node: Node<'a>, indent: usize) -> String {
        let text = self.node_text(node);
        if !text.contains('\n') || !text.contains('[') {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        }

        let Some(open) = text.find('[') else {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        };
        let Some(close) = text.rfind(']') else {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        };

        let prefix = normalize_statement(text[..open].trim().trim_end_matches(';'));
        let options = &text[open + 1..close];
        let mut out = format!("{}{} [", indent_text(indent), prefix.trim_end());
        for line in options
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            out.push('\n');
            out.push_str(&indent_text(indent + 1));
            out.push_str(&normalize_multiline_option_line(line));
        }
        out.push('\n');
        out.push_str(&indent_text(indent));
        out.push_str("];");
        out
    }

    fn render_option(&self, node: Node<'a>, indent: usize) -> String {
        let text = self.node_text(node);
        if !text.contains('\n') || !text.contains('{') {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        }

        let Some(open) = text.find('{') else {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        };
        let Some(close) = text.rfind('}') else {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        };

        let prefix = normalize_option_prefix(text[..open].trim().trim_end_matches(';'));
        let body = &text[open + 1..close];
        let mut out = format!("{}{} {{", indent_text(indent), prefix.trim_end());
        for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
            out.push('\n');
            out.push_str(&indent_text(indent + 1));
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&indent_text(indent));
        out.push_str("};");
        out
    }

    fn render_rpc(&self, node: Node<'a>, indent: usize) -> String {
        let text = self.node_text(node);
        if !text.contains('{') {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        }

        let Some(open) = text.find('{') else {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        };
        let Some(close) = text.rfind('}') else {
            return format!("{}{}", indent_text(indent), normalize_statement(text));
        };

        let prefix = normalize_rpc_prefix(text[..open].trim().trim_end_matches(';'));
        let body = &text[open + 1..close];
        let mut out = format!("{}{} {{", indent_text(indent), prefix.trim_end());
        for statement in split_inline_statements(body) {
            out.push('\n');
            out.push_str(&indent_text(indent + 1));
            out.push_str(&normalize_statement(statement));
            out.push(';');
        }
        out.push('\n');
        out.push_str(&indent_text(indent));
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
                "comment"
                    if items.last().is_some_and(|item: &Item<'_>| {
                        item.node.end_position().row == child.start_position().row
                    }) =>
                {
                    if let Some(item) = items.last_mut() {
                        item.trailing_comments.push(child);
                    }
                }
                "comment" => comments.push(child),
                "empty_statement" => {}
                kind if is_name_or_body_kind(kind) => {}
                _ => {
                    flush_detached_comments(&mut items, &mut comments, child);
                    items.push(Item {
                        node: child,
                        leading_comments: std::mem::take(&mut comments),
                        trailing_comments: Vec::new(),
                    });
                }
            }
        }
        for comment in comments {
            items.push(Item {
                node: comment,
                leading_comments: Vec::new(),
                trailing_comments: Vec::new(),
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

fn normalize_option_prefix(text: &str) -> String {
    normalize_statement(text).replace("option(", "option (")
}

fn normalize_rpc_prefix(text: &str) -> String {
    normalize_statement(text).replace(" returns(", " returns (")
}

fn normalize_multiline_option_line(line: &str) -> String {
    let has_comma = line.trim_end().ends_with(',');
    let mut normalized = normalize_statement(line.trim_end_matches(','));
    if has_comma {
        normalized.push(',');
    }
    normalized
}

fn normalize_reserved_or_extensions(text: &str) -> String {
    normalize_range_keywords(&normalize_statement(text))
}

fn normalize_range_keywords(text: &str) -> String {
    let mut out = String::new();
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index..].starts_with(&['t', 'o'])
            && index > 0
            && index + 2 < chars.len()
            && chars[index - 1].is_ascii_digit()
            && (chars[index + 2].is_ascii_digit()
                || chars[index + 2].is_ascii_alphabetic()
                || chars[index + 2].is_whitespace())
        {
            if !out.ends_with(' ') {
                out.push(' ');
            }
            out.push_str("to");
            if index + 2 < chars.len() && chars[index + 2] != ' ' {
                out.push(' ');
            }
            index += 2;
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

fn split_inline_statements(text: &str) -> Vec<&str> {
    let statements = text
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .collect::<Vec<_>>();
    if statements.is_empty() {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect()
    } else {
        statements
    }
}

fn append_trailing_comments(out: &mut String, comments: &[Node<'_>], source: &str) {
    const TRAILING_COMMENT_COLUMN: usize = 48;
    for comment in comments {
        let text = node_text(*comment, source).trim();
        if !text.is_empty() {
            let current_column = out.rsplit('\n').next().map(str::len).unwrap_or(0);
            let spaces = TRAILING_COMMENT_COLUMN
                .saturating_sub(current_column)
                .max(2);
            out.push_str(&" ".repeat(spaces));
            out.push_str(text);
        }
    }
}

fn flush_detached_comments<'a>(
    items: &mut Vec<Item<'a>>,
    comments: &mut Vec<Node<'a>>,
    next_node: Node<'a>,
) {
    if comments
        .last()
        .is_some_and(|comment| next_node.start_position().row > comment.end_position().row + 1)
    {
        for comment in std::mem::take(comments) {
            items.push(Item {
                node: comment,
                leading_comments: Vec::new(),
                trailing_comments: Vec::new(),
            });
        }
    }
}

fn indent_text(indent: usize) -> String {
    "  ".repeat(indent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_sorting_imports_fields_and_declarations() {
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
        assert!(output.find("message A").unwrap() < output.find("message Z").unwrap());
        assert!(output.contains("message Z {\n  string a = 1;\n  string b = 2;\n}"));
    }

    #[test]
    fn can_skip_all_sorting() {
        let input = r#"syntax = "proto3";
import "z.proto";
import "a.proto";
message Z { string b = 2; string a = 1; }
message A {}
"#;
        let output = format_proto(
            input,
            FormatOptions {
                sort_imports: false,
                sort_fields: false,
                sort_declarations: false,
                ..FormatOptions::default()
            },
        )
        .unwrap();
        assert!(
            output.find("import \"z.proto\"").unwrap() < output.find("import \"a.proto\"").unwrap()
        );
        assert!(output.find("message Z").unwrap() < output.find("message A").unwrap());
        assert!(output.find("string b = 2").unwrap() < output.find("string a = 1").unwrap());
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
    fn keeps_inline_field_options_compact() {
        let input = r#"syntax = "proto3";
message Foo { string name = 1 [deprecated=true, json_name="fullName"]; }
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(output.contains("string name = 1 [deprecated = true, json_name = \"fullName\"];"));
    }

    #[test]
    fn formats_multiline_field_options() {
        let input = r#"syntax = "proto3";
message Foo {
  string name = 1 [
      deprecated=true,
    json_name="fullName"
  ];
}
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(output.contains(
            "string name = 1 [\n    deprecated = true,\n    json_name = \"fullName\"\n  ];"
        ));
    }

    #[test]
    fn formats_reserved_and_extensions_ranges() {
        let input = r#"syntax = "proto3";
message Foo { reserved 2to5, 9; extensions 100to max; }
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(output.contains("reserved 2 to 5, 9;"));
        assert!(output.contains("extensions 100 to max;"));
    }

    #[test]
    fn formats_nested_blocks_from_cst() {
        let input = r#"syntax = "proto3";
message Outer { message Z {} message A {} enum E { TWO = 2; ONE = 1; } string b=2; string a=1; }
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(output.contains("message A {}") || output.contains("message A {\n}"));
        assert!(output.find("message A").unwrap() < output.find("message Z").unwrap());
        assert!(output.find("string a = 1").unwrap() < output.find("string b = 2").unwrap());
    }

    #[test]
    fn preserves_trailing_comments_with_sorted_items() {
        let input = r#"syntax = "proto3";
message Foo { string b = 2; // b field
string a = 1; // a field
}
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(output.contains("string a = 1;                                 // a field"));
        assert!(output.contains("string b = 2;                                 // b field"));
        assert!(output.find("string a = 1").unwrap() < output.find("string b = 2").unwrap());
    }

    #[test]
    fn does_not_move_detached_comments_with_sorted_declarations() {
        let input = r#"syntax = "proto3";
// detached

message Z {}
message A {}
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(output.find("// detached").unwrap() < output.find("message A").unwrap());
        assert!(output.find("// detached").unwrap() < output.find("message Z").unwrap());
    }

    #[test]
    fn formats_multiline_option_blocks_and_rpc_bodies() {
        let input = r#"syntax = "proto3";
message Foo {
  option (demo.option) = {
      enabled: true
    name: "foo"
  };
}
service FooService { rpc Get (Foo) returns (Foo) { option deprecated=true; } }
"#;
        let output = format_proto(input, FormatOptions::default()).unwrap();
        assert!(
            output.contains("option (demo.option) = {\n    enabled: true\n    name: \"foo\"\n  };")
        );
        assert!(
            output.contains("rpc Get(Foo) returns (Foo) {\n    option deprecated = true;\n  }")
        );
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(format_proto("message Foo { string name = ; }", FormatOptions::default()).is_err());
    }
}
