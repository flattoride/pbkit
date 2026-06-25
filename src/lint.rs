use anyhow::Result;

use crate::{
    proto_fmt::{FormatOptions, format_proto},
    syntax::syntax_diagnostics,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LintOptions {
    pub check_sort: bool,
}

impl Default for LintOptions {
    fn default() -> Self {
        Self { check_sort: true }
    }
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

    if lint_options.check_sort {
        let formatted = format_proto(source, FormatOptions::default())?;
        if formatted != source {
            return Ok(vec![LintDiagnostic {
                line: 1,
                column: 1,
                rule: "format".into(),
                severity: Severity::Warning,
                message: "file is not in canonical pbkit fmt order".into(),
            }]);
        }
    }

    Ok(Vec::new())
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
        let input = "syntax = \"proto3\";\nmessage Z {}\nmessage A {}\n";
        assert!(
            !lint_proto(input, LintOptions::default())
                .unwrap()
                .is_empty()
        );
        assert!(
            lint_proto(input, LintOptions { check_sort: false })
                .unwrap()
                .is_empty()
        );
    }
}
