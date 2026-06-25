use anyhow::{Context, Result};
use tree_sitter::{Node, Parser, Tree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxDiagnostic {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

pub fn parse_proto(source: &str) -> Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_proto::LANGUAGE.into())
        .context("failed to load protobuf tree-sitter grammar")?;
    parser
        .parse(source, None)
        .context("failed to parse protobuf source")
}

pub fn syntax_diagnostics(source: &str) -> Result<Vec<SyntaxDiagnostic>> {
    let tree = parse_proto(source)?;
    let mut diagnostics = Vec::new();
    collect_error_nodes(tree.root_node(), &mut diagnostics);
    Ok(diagnostics)
}

pub fn ensure_syntax(source: &str) -> Result<()> {
    let diagnostics = syntax_diagnostics(source)?;
    if diagnostics.is_empty() {
        return Ok(());
    }

    let first = &diagnostics[0];
    anyhow::bail!(
        "protobuf syntax error at {}:{}: {}",
        first.line,
        first.column,
        first.message
    )
}

fn collect_error_nodes(node: Node<'_>, diagnostics: &mut Vec<SyntaxDiagnostic>) {
    if node.is_error() || node.is_missing() {
        let position = node.start_position();
        diagnostics.push(SyntaxDiagnostic {
            line: position.row + 1,
            column: position.column + 1,
            message: if node.is_missing() {
                format!("missing {}", node.kind())
            } else {
                "parse error".into()
            },
        });
    }

    if !node.has_error() {
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_error_nodes(child, diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_proto_syntax_errors() {
        let diagnostics = syntax_diagnostics("message Foo { string name = ; }").unwrap();
        assert!(!diagnostics.is_empty());
    }

    #[test]
    fn accepts_valid_proto() {
        let diagnostics =
            syntax_diagnostics("syntax = \"proto3\";\nmessage Foo { string name = 1; }\n").unwrap();
        assert!(diagnostics.is_empty());
    }
}
