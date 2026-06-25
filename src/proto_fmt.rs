use anyhow::Result;

use crate::{
    proto_sort::{SortKey, sort_proto},
    syntax::ensure_syntax,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub sort: bool,
    pub field_key: SortKey,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            sort: true,
            field_key: SortKey::Number,
        }
    }
}

pub fn format_proto(source: &str, options: FormatOptions) -> Result<String> {
    ensure_syntax(source)?;

    let mut formatted = sort_imports(source);
    if options.sort {
        formatted = sort_proto(&formatted, options.field_key)?;
    }
    ensure_syntax(&formatted)?;
    Ok(formatted)
}

fn sort_imports(source: &str) -> String {
    let mut lines = source.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut i = 0;
    while i < lines.len() {
        if !is_plain_import_line(&lines[i]) {
            i += 1;
            continue;
        }

        let start = i;
        while i < lines.len() && is_plain_import_line(&lines[i]) {
            i += 1;
        }
        lines[start..i].sort();
    }

    let mut output = lines.join("\n");
    if source.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn is_plain_import_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("import ") && trimmed.ends_with(';') && !trimmed.contains("//")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_sorting_imports_and_declarations() {
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
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(format_proto("message Foo { string name = ; }", FormatOptions::default()).is_err());
    }
}
