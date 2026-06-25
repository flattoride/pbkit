use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::{lint::LintOptions, proto_fmt::FormatOptions, proto_sort::SortKey};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectConfig {
    pub fmt: FmtConfig,
    pub lint: LintConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FmtConfig {
    pub sort: Option<bool>,
    pub import_sort: Option<bool>,
    pub declaration_sort: Option<bool>,
    pub field_sort: Option<SortKey>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LintConfig {
    pub sort: Option<bool>,
}

impl ProjectConfig {
    pub fn format_options(
        &self,
        without_sort: bool,
        field_key_override: Option<SortKey>,
    ) -> FormatOptions {
        let mut options = FormatOptions::default();
        if let Some(sort) = self.fmt.sort {
            options.sort_imports = sort;
            options.sort_fields = sort;
            options.sort_declarations = sort;
        }
        if let Some(import_sort) = self.fmt.import_sort {
            options.sort_imports = import_sort;
        }
        if let Some(declaration_sort) = self.fmt.declaration_sort {
            options.sort_declarations = declaration_sort;
        }
        if let Some(field_key) = self.fmt.field_sort {
            options.field_key = field_key;
        }
        if let Some(field_key) = field_key_override {
            options.field_key = field_key;
        }
        if without_sort {
            options.sort_imports = false;
            options.sort_fields = false;
            options.sort_declarations = false;
        }
        options
    }

    pub fn lint_options(&self, without_sort: bool) -> LintOptions {
        let mut format_options = self.format_options(false, None);
        if let Some(sort) = self.lint.sort {
            format_options.sort_imports = sort;
            format_options.sort_fields = sort;
            format_options.sort_declarations = sort;
        }
        if without_sort {
            format_options.sort_imports = false;
            format_options.sort_fields = false;
            format_options.sort_declarations = false;
        }
        LintOptions { format_options }
    }
}

pub fn load_config(path: Option<&Path>) -> Result<ProjectConfig> {
    let Some(path) = path.map(Path::to_path_buf).or_else(find_default_config) else {
        return Ok(ProjectConfig::default());
    };
    let source = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    parse_config(&source).with_context(|| format!("failed to parse config {}", path.display()))
}

pub fn find_default_config() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("pbkit.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub fn parse_config(source: &str) -> Result<ProjectConfig> {
    let mut config = ProjectConfig::default();
    let mut section = String::new();

    for (line_index, raw_line) in source.lines().enumerate() {
        let line_number = line_index + 1;
        let line = strip_comment(raw_line).trim().to_owned();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') {
                bail!("unterminated section header at line {line_number}");
            }
            section = line[1..line.len() - 1].trim().to_owned();
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            bail!("expected key = value at line {line_number}");
        };
        let key = key.trim();
        let value = value.trim();
        match (section.as_str(), key) {
            ("fmt", "sort") => config.fmt.sort = Some(parse_bool(value, line_number)?),
            ("fmt", "import_sort") => {
                config.fmt.import_sort = Some(parse_bool(value, line_number)?);
            }
            ("fmt", "declaration_sort") => {
                config.fmt.declaration_sort = Some(parse_bool(value, line_number)?);
            }
            ("fmt", "field_sort") => {
                config.fmt.field_sort = Some(parse_sort_key(value, line_number)?)
            }
            ("lint", "sort") => config.lint.sort = Some(parse_bool(value, line_number)?),
            _ => {}
        }
    }

    Ok(config)
}

fn parse_bool(value: &str, line_number: usize) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("expected boolean at line {line_number}"),
    }
}

fn parse_sort_key(value: &str, line_number: usize) -> Result<SortKey> {
    match unquote(value) {
        "number" => Ok(SortKey::Number),
        "name" => Ok(SortKey::Name),
        _ => bail!("expected field_sort to be \"number\" or \"name\" at line {line_number}"),
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn strip_comment(line: &str) -> String {
    let mut in_quote = None;
    let mut out = String::new();
    for ch in line.chars() {
        match ch {
            '"' | '\'' if in_quote == Some(ch) => in_quote = None,
            '"' | '\'' if in_quote.is_none() => in_quote = Some(ch),
            '#' if in_quote.is_none() => break,
            _ => {}
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fmt_and_lint_defaults() {
        let config = parse_config(
            r#"
[fmt]
sort = true
field_sort = "name"
declaration_sort = false

[lint]
sort = false
disabled = ["field-name"]
"#,
        )
        .unwrap();
        assert_eq!(config.fmt.sort, Some(true));
        assert_eq!(config.fmt.field_sort, Some(SortKey::Name));
        assert_eq!(config.fmt.declaration_sort, Some(false));
        assert_eq!(config.lint.sort, Some(false));
    }

    #[test]
    fn applies_cli_overrides_after_config() {
        let config = parse_config(
            r#"
[fmt]
sort = true
field_sort = "name"
"#,
        )
        .unwrap();
        let options = config.format_options(true, Some(SortKey::Number));
        assert!(!options.sort_imports);
        assert!(!options.sort_fields);
        assert!(!options.sort_declarations);
        assert_eq!(options.field_key, SortKey::Number);
    }
}
