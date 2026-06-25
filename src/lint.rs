use anyhow::Result;
use tree_sitter::Node;

use crate::{
    proto_fmt::{FormatOptions, format_proto},
    syntax::{parse_proto, syntax_diagnostics},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintDiagnostic {
    pub line: usize,
    pub column: usize,
    pub rule: String,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LintOptions {
    pub format_options: FormatOptions,
}

pub fn lint_proto(source: &str, lint_options: LintOptions) -> Result<Vec<LintDiagnostic>> {
    let syntax = syntax_diagnostics(source)?;
    if !syntax.is_empty() {
        return Ok(syntax
            .into_iter()
            .map(|diagnostic| LintDiagnostic {
                line: diagnostic.line,
                column: diagnostic.column,
                rule: "parse".into(),
                severity: Severity::Error,
                message: diagnostic.message,
            })
            .collect());
    }

    let tree = parse_proto(source)?;
    let root = tree.root_node();
    let mut diagnostics = Vec::new();
    lint_file_header(root, source, &mut diagnostics);
    lint_names(root, source, &mut diagnostics);

    let formatted = format_proto(source, lint_options.format_options)?;
    if formatted != source {
        let checks_order = lint_options.format_options.sort_imports
            || lint_options.format_options.sort_fields
            || lint_options.format_options.sort_declarations;
        diagnostics.push(LintDiagnostic {
            line: 1,
            column: 1,
            rule: "format".into(),
            severity: Severity::Warning,
            message: if checks_order {
                "file is not in canonical pbkit fmt layout/order".into()
            } else {
                "file is not in canonical pbkit fmt layout".into()
            },
        });
    }

    Ok(diagnostics)
}

fn lint_file_header(root: Node<'_>, source: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    let children = named_children(root);
    let has_syntax = children
        .iter()
        .any(|child| matches!(child.kind(), "syntax" | "edition"));
    let has_package = children.iter().any(|child| child.kind() == "package");

    if !has_syntax {
        diagnostics.push(LintDiagnostic {
            line: 1,
            column: 1,
            rule: "missing-syntax".into(),
            severity: Severity::Warning,
            message: "file should declare syntax or edition".into(),
        });
    }

    if !has_package {
        let position = children
            .iter()
            .find(|child| matches!(child.kind(), "message" | "enum" | "service" | "extend"))
            .map(|child| child.start_position());
        diagnostics.push(LintDiagnostic {
            line: position.map_or(1, |point| point.row + 1),
            column: position.map_or(1, |point| point.column + 1),
            rule: "missing-package".into(),
            severity: Severity::Warning,
            message: "file should declare a package".into(),
        });
    }

    if source.contains("syntax = \"proto3\"") || source.contains("syntax=\"proto3\"") {
        for node in descendants(root)
            .into_iter()
            .filter(|node| node.kind() == "field")
        {
            let text = node_text(node, source).trim_start();
            if text.starts_with("required ") {
                push_node_diagnostic(
                    diagnostics,
                    node,
                    "proto3-required",
                    Severity::Error,
                    "proto3 fields must not use required",
                );
            }
        }
    }
}

fn lint_names(root: Node<'_>, source: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    for node in descendants(root) {
        match node.kind() {
            "message" => lint_named_child(
                node,
                "message_name",
                source,
                diagnostics,
                "message-name",
                "message names should be PascalCase",
                is_pascal_case,
            ),
            "enum" => lint_named_child(
                node,
                "enum_name",
                source,
                diagnostics,
                "enum-name",
                "enum names should be PascalCase",
                is_pascal_case,
            ),
            "service" => lint_named_child(
                node,
                "service_name",
                source,
                diagnostics,
                "service-name",
                "service names should be PascalCase",
                is_pascal_case,
            ),
            "rpc" => {
                if let Some(name) = child_by_kind(node, "rpc_name").or_else(|| {
                    named_children(node)
                        .into_iter()
                        .find(|child| child.kind() == "identifier")
                }) {
                    lint_name_node(
                        name,
                        source,
                        diagnostics,
                        "rpc-name",
                        "rpc names should be PascalCase",
                        is_pascal_case,
                    );
                }
            }
            "field" | "map_field" | "oneof_field" => {
                if let Some(name) = field_name_node(node) {
                    lint_name_node(
                        name,
                        source,
                        diagnostics,
                        "field-name",
                        "field names should be lower_snake_case",
                        is_lower_snake_case,
                    );
                }
            }
            "enum_field" => {
                if let Some(name) = named_children(node)
                    .into_iter()
                    .find(|child| child.kind() == "identifier")
                {
                    lint_name_node(
                        name,
                        source,
                        diagnostics,
                        "enum-value-name",
                        "enum value names should be UPPER_SNAKE_CASE",
                        is_upper_snake_case,
                    );
                }
            }
            _ => {}
        }
    }
}

fn lint_named_child(
    node: Node<'_>,
    child_kind: &str,
    source: &str,
    diagnostics: &mut Vec<LintDiagnostic>,
    rule: &str,
    message: &str,
    predicate: fn(&str) -> bool,
) {
    if let Some(name) = child_by_kind(node, child_kind) {
        lint_name_node(name, source, diagnostics, rule, message, predicate);
    }
}

fn lint_name_node(
    node: Node<'_>,
    source: &str,
    diagnostics: &mut Vec<LintDiagnostic>,
    rule: &str,
    message: &str,
    predicate: fn(&str) -> bool,
) {
    let name = node_text(node, source).trim();
    if !name.is_empty() && !predicate(name) {
        push_node_diagnostic(diagnostics, node, rule, Severity::Warning, message);
    }
}

fn push_node_diagnostic(
    diagnostics: &mut Vec<LintDiagnostic>,
    node: Node<'_>,
    rule: &str,
    severity: Severity,
    message: &str,
) {
    let position = node.start_position();
    diagnostics.push(LintDiagnostic {
        line: position.row + 1,
        column: position.column + 1,
        rule: rule.into(),
        severity,
        message: message.into(),
    });
}

fn descendants(node: Node<'_>) -> Vec<Node<'_>> {
    let mut nodes = Vec::new();
    collect_descendants(node, &mut nodes);
    nodes
}

fn collect_descendants<'a>(node: Node<'a>, nodes: &mut Vec<Node<'a>>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor).filter(|child| child.is_named()) {
        nodes.push(child);
        collect_descendants(child, nodes);
    }
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.is_named())
        .collect()
}

fn child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == kind)
}

fn field_name_node(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.is_named() && child.kind() == "identifier")
        .last()
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn is_pascal_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_uppercase())
        && chars.all(|ch| ch.is_ascii_alphanumeric())
        && !name.contains('_')
}

fn is_lower_snake_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_lowercase())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && !name.contains("__")
        && !name.ends_with('_')
}

fn is_upper_snake_case(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
        && name
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        && !name.contains("__")
        && !name.ends_with('_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_parse_errors() {
        let diagnostics =
            lint_proto("message Foo { string name = ; }", LintOptions::default()).unwrap();
        assert_eq!(diagnostics[0].rule, "parse");
    }

    #[test]
    fn can_skip_sort_checks() {
        let input = "syntax = \"proto3\";\npackage demo;\n\nmessage Z {}\n\nmessage A {}\n";
        assert!(
            !lint_proto(input, LintOptions::default())
                .unwrap()
                .is_empty()
        );
        assert!(
            lint_proto(
                input,
                LintOptions {
                    format_options: FormatOptions {
                        sort_imports: false,
                        sort_fields: false,
                        sort_declarations: false,
                        ..FormatOptions::default()
                    }
                }
            )
            .unwrap()
            .iter()
            .all(|diagnostic| diagnostic.rule != "format")
        );
    }

    #[test]
    fn reports_default_header_and_name_rules() {
        let input = "message foo { string BadName = 1; enum bad_enum { bad = 0; } }\n";
        let diagnostics = lint_proto(
            input,
            LintOptions {
                format_options: FormatOptions {
                    sort_imports: false,
                    sort_fields: false,
                    sort_declarations: false,
                    ..FormatOptions::default()
                },
            },
        )
        .unwrap();
        let rules = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.rule.as_str())
            .collect::<Vec<_>>();
        assert!(rules.contains(&"missing-syntax"));
        assert!(rules.contains(&"missing-package"));
        assert!(rules.contains(&"message-name"));
        assert!(rules.contains(&"field-name"));
        assert!(rules.contains(&"enum-name"));
        assert!(rules.contains(&"enum-value-name"));
    }

    #[test]
    fn reports_proto3_required() {
        let input =
            "syntax = \"proto3\";\npackage demo;\nmessage Foo { required string name = 1; }\n";
        let diagnostics = lint_proto(
            input,
            LintOptions {
                format_options: FormatOptions {
                    sort_imports: false,
                    sort_fields: false,
                    sort_declarations: false,
                    ..FormatOptions::default()
                },
            },
        )
        .unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.rule == "proto3-required")
        );
    }
}
