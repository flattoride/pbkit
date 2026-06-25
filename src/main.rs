use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum, ValueHint};
use clap_complete::Shell;
use pbkit::{
    config::load_config,
    lint::{LintDiagnostic, LintOptions, Severity, lint_proto},
    path::{parse_path, select_many},
    proto_fmt::{FormatOptions, format_proto},
    proto_sort::{SortKey, sort_proto},
    reflect::{decode_to_json, load_pool},
    schema::{
        Candidate, enum_candidates, field_candidates, message_candidates, query_path_candidates,
    },
    wire::{decode_message, raw_bytes_from_json, to_json},
};
use prost_reflect::DescriptorPool;
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "A pragmatic protobuf sorting and binary query toolkit.",
    after_help = "Shell completions:
  zsh:  mkdir -p ~/.zfunc && pbkit completions zsh > ~/.zfunc/_pbkit
        then add `fpath=(~/.zfunc $fpath); autoload -Uz compinit; compinit` to ~/.zshrc
  bash: pbkit completions bash > pbkit.bash && source pbkit.bash
  fish: pbkit completions fish > ~/.config/fish/completions/pbkit.fish"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Sort message/enum declarations and field lines in .proto text. Prefer `fmt` for new use.
    Sort {
        /// Proto files to sort. Reads stdin when omitted.
        #[arg(value_hint = ValueHint::FilePath)]
        files: Vec<PathBuf>,
        /// Write sorted output back to each input file.
        #[arg(short, long)]
        write: bool,
        /// Sort fields by tag number or field name.
        #[arg(long, default_value = "number")]
        fields: FieldSort,
    },
    /// Format proto files after protobuf syntax validation.
    #[command(after_help = "Default fmt behavior:
  - normalizes layout with two-space indentation, stable spacing, and one trailing newline
  - sorts imports
  - sorts fields and enum values by number, or by name with --fields name
  - sorts message, enum, service, and extend declarations

Use --without-sort to keep all original import, declaration, field, and enum value order while still normalizing layout.
Use --check --diff in CI to fail on formatting changes and print a unified diff.")]
    Fmt {
        /// Proto files or directories to format. Directories are searched recursively for .proto files. Reads stdin when omitted.
        #[arg(value_hint = ValueHint::FilePath)]
        files: Vec<PathBuf>,
        /// Write formatted output back to each input file.
        #[arg(short, long)]
        write: bool,
        /// Exit with a non-zero status when formatting would change output.
        #[arg(long)]
        check: bool,
        /// Print a unified diff for files that would change. Cannot be used with --write.
        #[arg(long)]
        diff: bool,
        /// Config file to read. Defaults to pbkit.toml in the current directory or an ancestor.
        #[arg(long, value_hint = ValueHint::FilePath)]
        config: Option<PathBuf>,
        /// Recurse into directory inputs. Directory inputs are recursive by default.
        #[arg(long)]
        recursive: bool,
        /// Skip all sorting. Keeps import, declaration, field, and enum value order while still normalizing layout.
        #[arg(long)]
        without_sort: bool,
        /// Sort fields and enum values by tag number or name. Overrides [fmt].field_sort. Ignored with --without-sort.
        #[arg(long)]
        fields: Option<FieldSort>,
    },
    /// Lint proto files. By default this includes pbkit fmt layout/order checks.
    #[command(after_help = "Default lint behavior:
  - validates protobuf syntax with tree-sitter
  - checks for syntax or edition declaration
  - checks for package declaration
  - checks message, enum, service, rpc, field, and enum value naming
  - rejects required fields under proto3
  - checks canonical pbkit fmt layout/order

Use --without-sort to keep the default lint rules and layout checks, but ignore import, declaration, field, and enum value order.")]
    Lint {
        /// Proto files or directories to lint. Directories are searched recursively for .proto files.
        #[arg(required = true, value_hint = ValueHint::FilePath)]
        files: Vec<PathBuf>,
        /// Config file to read. Defaults to pbkit.toml in the current directory or an ancestor.
        #[arg(long, value_hint = ValueHint::FilePath)]
        config: Option<PathBuf>,
        /// Recurse into directory inputs. Directory inputs are recursive by default.
        #[arg(long)]
        recursive: bool,
        /// Skip sorting in the fmt check. Default lint rules and layout checks still run.
        #[arg(long)]
        without_sort: bool,
    },
    /// Decode a protobuf binary without descriptors into numbered wire fields.
    Decode {
        /// Optional protobuf binary input. Reads stdin when omitted.
        #[arg(value_hint = ValueHint::FilePath)]
        input: Option<PathBuf>,
    },
    /// Query a protobuf binary using a small JSONPath-like selector.
    #[command(after_help = "Path syntax:
  $.items[0].id      select one array item
  $.items[*].id      select every array item
  $.items.id         shorthand for applying .id to every item when items is an array
  $.labels['foo']    select an object/map key
  $.*                select every object value

Multiple matches are returned as a JSON array.")]
    Query {
        /// Path such as '$.items[0].id' for descriptors or '$.2[0].message.1[0]' for raw wire data.
        path: String,
        /// Optional protobuf binary input. Reads stdin when omitted.
        #[arg(value_hint = ValueHint::FilePath)]
        input: Option<PathBuf>,
        /// Fully-qualified message name used with --descriptor-set or --proto. Optional when descriptors contain one message.
        #[arg(short, long)]
        message: Option<String>,
        /// Binary FileDescriptorSet produced by protoc --descriptor_set_out.
        #[arg(long, value_hint = ValueHint::FilePath)]
        descriptor_set: Option<PathBuf>,
        /// Proto source file. Can be repeated.
        #[arg(long = "proto", value_hint = ValueHint::FilePath)]
        proto_files: Vec<PathBuf>,
        /// Proto include path. Can be repeated. Defaults to current directory.
        #[arg(short = 'I', long = "include", value_hint = ValueHint::DirPath)]
        includes: Vec<PathBuf>,
        /// Output selected data as json, raw bytes, hex, or base64.
        #[arg(short, long, default_value = "json")]
        format: OutputFormat,
    },
    /// Generate shell completion scripts.
    #[command(after_help = "Install example:
  mkdir -p ~/.zfunc
  pbkit completions zsh > ~/.zfunc/_pbkit

Then ensure ~/.zshrc contains:
  fpath=(~/.zfunc $fpath)
  autoload -Uz compinit
  compinit

The zsh script includes dynamic proto-aware completion for --proto, --message, fields, and query paths.")]
    Completions {
        /// Shell to generate completions for.
        shell: Shell,
    },
    /// Print proto-aware completion candidates, one per line.
    Complete {
        /// Candidate type to print.
        target: CompleteTarget,
        /// Candidate prefix. For query-path, pass the partial path such as '$.user.n'.
        #[arg(long, default_value = "")]
        prefix: String,
        /// Fully-qualified message name used for fields or query-path. Optional when descriptors contain one message.
        #[arg(short, long)]
        message: Option<String>,
        /// Binary FileDescriptorSet produced by protoc --descriptor_set_out.
        #[arg(long, value_hint = ValueHint::FilePath)]
        descriptor_set: Option<PathBuf>,
        /// Proto source file. Can be repeated.
        #[arg(long = "proto", value_hint = ValueHint::FilePath)]
        proto_files: Vec<PathBuf>,
        /// Proto include path. Can be repeated. Defaults to current directory.
        #[arg(short = 'I', long = "include", value_hint = ValueHint::DirPath)]
        includes: Vec<PathBuf>,
        /// Include tab-separated type details after each value.
        #[arg(long)]
        details: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_diff_shows_changed_proto_lines() {
        let diff = unified_diff(
            "api.proto",
            "syntax = \"proto3\";\nmessage Foo { string b = 2; string a = 1; }\n",
            "syntax = \"proto3\";\n\nmessage Foo {\n  string a = 1;\n  string b = 2;\n}\n",
        );
        assert!(diff.contains("--- api.proto\n+++ api.proto\n@@ -1,2 +1,6 @@\n"));
        assert!(diff.contains("-message Foo { string b = 2; string a = 1; }\n"));
        assert!(diff.contains("+message Foo {\n"));
        assert!(diff.contains("+  string a = 1;\n"));
    }

    #[test]
    fn unified_diff_keeps_small_context_window() {
        let old = "a\nb\nc\nd\ne\nf\ng\n";
        let new = "a\nb\nc\nD\ne\nf\ng\n";
        let diff = unified_diff("x.proto", old, new);
        assert!(diff.contains("@@ -1,7 +1,7 @@\n"));
        assert!(diff.contains("-d\n+D\n"));
    }

    #[test]
    fn proto_file_completion_recurses_include_roots() {
        let root = std::env::temp_dir().join(format!("pbkit-proto-files-{}", std::process::id()));
        let nested = root.join("api/v1");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("user.proto"), "syntax = \"proto3\";\n").unwrap();

        let candidates = proto_file_candidates(std::slice::from_ref(&root), "api/").unwrap();
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.value.ends_with("api/v1/user.proto"))
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum FieldSort {
    Number,
    Name,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Json,
    Raw,
    Hex,
    Base64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CompleteTarget {
    ProtoFiles,
    Messages,
    Enums,
    Fields,
    QueryPath,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sort {
            files,
            write,
            fields,
        } => run_sort(files, write, fields.into()),
        Command::Fmt {
            files,
            write,
            check,
            diff,
            config,
            recursive: _,
            without_sort,
            fields,
        } => run_fmt(
            files,
            write,
            check,
            diff,
            load_config(config.as_deref())?.format_options(without_sort, fields.map(Into::into)),
        ),
        Command::Lint {
            files,
            config,
            recursive: _,
            without_sort,
        } => run_lint(
            files,
            load_config(config.as_deref())?.lint_options(without_sort),
        ),
        Command::Decode { input } => {
            let bytes = read_input(input.as_ref())?;
            let decoded = to_json(&decode_message(&bytes)?);
            print_json(&decoded)
        }
        Command::Query {
            path,
            input,
            message,
            descriptor_set,
            proto_files,
            includes,
            format,
        } => run_query(
            &path,
            input,
            message,
            descriptor_set,
            proto_files,
            includes,
            format,
        ),
        Command::Completions { shell } => run_completions(shell),
        Command::Complete {
            target,
            prefix,
            message,
            descriptor_set,
            proto_files,
            includes,
            details,
        } => run_complete(
            target,
            &prefix,
            message,
            descriptor_set,
            proto_files,
            includes,
            details,
        ),
    }
}

fn run_sort(files: Vec<PathBuf>, write: bool, field_key: SortKey) -> Result<()> {
    if files.is_empty() {
        if write {
            bail!("--write requires at least one input file");
        }
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("failed to read stdin")?;
        print!("{}", sort_proto(&source, field_key)?);
        return Ok(());
    }

    for file in files {
        let source = std::fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        let sorted = sort_proto(&source, field_key)?;
        if write {
            std::fs::write(&file, sorted)
                .with_context(|| format!("failed to write {}", file.display()))?;
        } else {
            print!("{sorted}");
        }
    }
    Ok(())
}

fn run_fmt(
    files: Vec<PathBuf>,
    write: bool,
    check: bool,
    diff: bool,
    options: FormatOptions,
) -> Result<()> {
    if write && diff {
        bail!("--diff cannot be used with --write");
    }

    if files.is_empty() {
        if write || check || diff {
            bail!("--write, --check, and --diff require at least one input file");
        }
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("failed to read stdin")?;
        print!("{}", format_proto(&source, options)?);
        return Ok(());
    }

    let mut changed = false;
    for file in expand_proto_inputs(files)? {
        let source = std::fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        let formatted = format_proto(&source, options)
            .with_context(|| format!("failed to format {}", file.display()))?;
        if formatted != source {
            changed = true;
            if diff {
                print!(
                    "{}",
                    unified_diff(&file.display().to_string(), &source, &formatted)
                );
            }
        }
        if write {
            std::fs::write(&file, formatted)
                .with_context(|| format!("failed to write {}", file.display()))?;
        } else if !check && !diff {
            print!("{formatted}");
        }
    }

    if check && changed {
        bail!("format check failed");
    }
    Ok(())
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let old_lines = diff_lines(old);
    let new_lines = diff_lines(new);
    let hunk = diff_hunk(&old_lines, &new_lines);

    let mut out = String::new();
    out.push_str(&format!("--- {path}\n"));
    out.push_str(&format!("+++ {path}\n"));
    out.push_str(&format!(
        "@@ -{} +{} @@\n",
        format_diff_range(hunk.old_start, hunk.old_count),
        format_diff_range(hunk.new_start, hunk.new_count)
    ));
    for edit in hunk.edits {
        match edit {
            DiffEdit::Equal(line) => {
                out.push(' ');
                out.push_str(line);
                out.push('\n');
            }
            DiffEdit::Delete(line) => {
                out.push('-');
                out.push_str(line);
                out.push('\n');
            }
            DiffEdit::Insert(line) => {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

fn diff_lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

fn format_diff_range(start: usize, count: usize) -> String {
    if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffEdit<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
struct DiffHunk<'a> {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    edits: Vec<DiffEdit<'a>>,
}

fn diff_hunk<'a>(old: &[&'a str], new: &[&'a str]) -> DiffHunk<'a> {
    const CONTEXT: usize = 3;

    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        prefix += 1;
    }

    let mut suffix = 0;
    while suffix < old.len().saturating_sub(prefix)
        && suffix < new.len().saturating_sub(prefix)
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let hunk_old_start = prefix.saturating_sub(CONTEXT);
    let hunk_new_start = prefix.saturating_sub(CONTEXT);
    let old_change_end = old.len() - suffix;
    let new_change_end = new.len() - suffix;
    let hunk_old_end = (old_change_end + CONTEXT).min(old.len());
    let hunk_new_end = (new_change_end + CONTEXT).min(new.len());
    let mut edits = Vec::new();

    for line in &old[hunk_old_start..prefix] {
        edits.push(DiffEdit::Equal(line));
    }
    for line in &old[prefix..old_change_end] {
        edits.push(DiffEdit::Delete(line));
    }
    for line in &new[prefix..new_change_end] {
        edits.push(DiffEdit::Insert(line));
    }
    let suffix_context_len = (hunk_old_end - old_change_end).min(hunk_new_end - new_change_end);
    for index in 0..suffix_context_len {
        edits.push(DiffEdit::Equal(old[old_change_end + index]));
    }

    if edits.is_empty() && hunk_old_start < old.len() && hunk_new_start < new.len() {
        edits.push(DiffEdit::Equal(old[hunk_old_start]));
    }

    DiffHunk {
        old_start: hunk_old_start + 1,
        old_count: hunk_old_end - hunk_old_start,
        new_start: hunk_new_start + 1,
        new_count: hunk_new_end - hunk_new_start,
        edits,
    }
}

fn run_lint(files: Vec<PathBuf>, options: LintOptions) -> Result<()> {
    let mut failed = false;
    for file in expand_proto_inputs(files)? {
        let source = std::fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        let diagnostics = lint_proto(&source, options)
            .with_context(|| format!("failed to lint {}", file.display()))?;
        if !diagnostics.is_empty() {
            failed = true;
        }
        print_lint_diagnostics(&file, &diagnostics);
    }

    if failed {
        bail!("lint failed");
    }
    Ok(())
}

fn expand_proto_inputs(inputs: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for input in inputs {
        if input.is_dir() {
            collect_proto_files(&input, &mut files)?;
        } else {
            files.push(input);
        }
    }
    files.sort();
    Ok(files)
}

fn collect_proto_files(dir: &std::path::Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_proto_files(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("proto") {
            files.push(path);
        }
    }
    Ok(())
}

fn print_lint_diagnostics(file: &std::path::Path, diagnostics: &[LintDiagnostic]) {
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!(
            "{}:{}:{}: {}[{}] {}",
            file.display(),
            diagnostic.line,
            diagnostic.column,
            severity,
            diagnostic.rule,
            diagnostic.message
        );
    }
}

fn run_query(
    query: &str,
    input: Option<PathBuf>,
    message: Option<String>,
    descriptor_set: Option<PathBuf>,
    proto_files: Vec<PathBuf>,
    includes: Vec<PathBuf>,
    format: OutputFormat,
) -> Result<()> {
    let bytes = read_input(input.as_ref())?;
    let root = if descriptor_set.is_some() || !proto_files.is_empty() {
        let pool = load_pool(descriptor_set.as_deref(), &proto_files, &includes)?;
        let message = resolve_message_name(&pool, message, "query")?;
        decode_to_json(&pool, &message, &bytes)?
    } else {
        to_json(&decode_message(&bytes)?)
    };

    let path = parse_path(query)?;
    let selected = select_many(&root, &path);
    if selected.is_empty() {
        bail!("path {query:?} did not match");
    }
    if selected.len() == 1 {
        write_value(selected[0], format)
    } else {
        let values = Value::Array(selected.into_iter().cloned().collect());
        write_value(&values, format)
    }
}

fn run_completions(shell: Shell) -> Result<()> {
    if shell == Shell::Bash {
        print!("{}", enhanced_bash_completion());
        return Ok(());
    }
    if shell == Shell::Zsh {
        print!("{}", enhanced_zsh_completion());
        return Ok(());
    }
    if shell == Shell::Fish {
        print!("{}", enhanced_fish_completion());
        return Ok(());
    }

    let mut command = Cli::command();
    clap_complete::generate(shell, &mut command, "pbkit", &mut io::stdout());
    Ok(())
}

fn enhanced_bash_completion() -> &'static str {
    r#"# Generated by `pbkit completions bash`.
# Install:
#   pbkit completions bash > pbkit.bash
#   source pbkit.bash
#
# This completion calls `pbkit complete ...` for proto-aware candidates.

_pbkit_schema_args=()

_pbkit_collect_schema_args() {
    _pbkit_schema_args=()
    local i word next
    for (( i = 1; i < COMP_CWORD; i++ )); do
        word="${COMP_WORDS[i]}"
        case "$word" in
            --descriptor-set|--proto|-I|--include)
                next="${COMP_WORDS[i+1]}"
                if [[ -n "$next" && "$next" != -* ]]; then
                    _pbkit_schema_args+=("$word" "$next")
                fi
                ;;
            --descriptor-set=*)
                _pbkit_schema_args+=(--descriptor-set "${word#--descriptor-set=}")
                ;;
            --proto=*)
                _pbkit_schema_args+=(--proto "${word#--proto=}")
                ;;
            --include=*)
                _pbkit_schema_args+=(-I "${word#--include=}")
                ;;
            -I?*)
                _pbkit_schema_args+=(-I "${word#-I}")
                ;;
        esac
    done
}

_pbkit_message_arg() {
    local i word next
    for (( i = 1; i < COMP_CWORD; i++ )); do
        word="${COMP_WORDS[i]}"
        case "$word" in
            -m|--message)
                next="${COMP_WORDS[i+1]}"
                if [[ -n "$next" && "$next" != -* ]]; then
                    printf '%s\n' "$next"
                    return
                fi
                ;;
            --message=*)
                printf '%s\n' "${word#--message=}"
                return
                ;;
        esac
    done
}

_pbkit_rows_to_replies() {
    local row value
    while IFS= read -r row; do
        value="${row%%$'\t'*}"
        [[ -n "$value" ]] && COMPREPLY+=("$value")
    done
}

_pbkit_proto_files() {
    _pbkit_collect_schema_args
    _pbkit_rows_to_replies < <("${COMP_WORDS[0]}" complete proto-files "${_pbkit_schema_args[@]}" --prefix "$cur" --details 2>/dev/null)
}

_pbkit_messages() {
    _pbkit_collect_schema_args
    _pbkit_rows_to_replies < <("${COMP_WORDS[0]}" complete messages "${_pbkit_schema_args[@]}" --prefix "$cur" --details 2>/dev/null)
}

_pbkit_fields() {
    _pbkit_collect_schema_args
    local message
    message="$(_pbkit_message_arg)"
    if [[ -n "$message" ]]; then
        _pbkit_rows_to_replies < <("${COMP_WORDS[0]}" complete fields "${_pbkit_schema_args[@]}" --message "$message" --prefix "$cur" --details 2>/dev/null)
    else
        _pbkit_rows_to_replies < <("${COMP_WORDS[0]}" complete fields "${_pbkit_schema_args[@]}" --prefix "$cur" --details 2>/dev/null)
    fi
}

_pbkit_query_paths() {
    _pbkit_collect_schema_args
    local message
    message="$(_pbkit_message_arg)"
    if [[ -n "$message" ]]; then
        _pbkit_rows_to_replies < <("${COMP_WORDS[0]}" complete query-path "${_pbkit_schema_args[@]}" --message "$message" --prefix "$cur" --details 2>/dev/null)
    else
        _pbkit_rows_to_replies < <("${COMP_WORDS[0]}" complete query-path "${_pbkit_schema_args[@]}" --prefix "$cur" --details 2>/dev/null)
    fi
}

_pbkit_complete_targets() {
    COMPREPLY=( $(compgen -W "proto-files messages enums fields query-path" -- "$cur") )
}

_pbkit_subcommands() {
    COMPREPLY=( $(compgen -W "sort fmt lint decode query completions complete help" -- "$cur") )
}

_pbkit() {
    COMPREPLY=()
    local cur prev subcmd target i
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    subcmd=""
    target=""

    for (( i = 1; i < COMP_CWORD; i++ )); do
        if [[ -z "$subcmd" && "${COMP_WORDS[i]}" != -* ]]; then
            subcmd="${COMP_WORDS[i]}"
            continue
        fi
        if [[ "$subcmd" == complete && -z "$target" && "${COMP_WORDS[i]}" != -* ]]; then
            target="${COMP_WORDS[i]}"
            continue
        fi
    done

    if [[ -z "$subcmd" ]]; then
        _pbkit_subcommands
        return
    fi

    case "$subcmd" in
        sort)
            case "$prev" in
                --fields) COMPREPLY=( $(compgen -W "number name" -- "$cur") ); return ;;
            esac
            if [[ "$cur" == --* ]]; then
                COMPREPLY=( $(compgen -W "--write --fields --help" -- "$cur") )
            else
                _pbkit_proto_files
            fi
            ;;
        fmt)
            case "$prev" in
                --fields) COMPREPLY=( $(compgen -W "number name" -- "$cur") ); return ;;
                --config) compopt -o default 2>/dev/null; return ;;
            esac
            if [[ "$cur" == --* ]]; then
                COMPREPLY=( $(compgen -W "--write --check --diff --config --recursive --without-sort --fields --help" -- "$cur") )
            else
                _pbkit_proto_files
            fi
            ;;
        lint)
            case "$prev" in
                --config) compopt -o default 2>/dev/null; return ;;
            esac
            if [[ "$cur" == --* ]]; then
                COMPREPLY=( $(compgen -W "--config --recursive --without-sort --help" -- "$cur") )
            else
                _pbkit_proto_files
            fi
            ;;
        decode)
            if [[ "$cur" == --* ]]; then
                COMPREPLY=( $(compgen -W "--help" -- "$cur") )
            else
                compopt -o default 2>/dev/null
            fi
            ;;
        query)
            case "$prev" in
                --proto) _pbkit_proto_files; return ;;
                -I|--include) compopt -o dirnames 2>/dev/null; return ;;
                --descriptor-set) compopt -o default 2>/dev/null; return ;;
                -m|--message) _pbkit_messages; return ;;
                -f|--format) COMPREPLY=( $(compgen -W "json raw hex base64" -- "$cur") ); return ;;
            esac
            if [[ "$cur" == --* ]]; then
                COMPREPLY=( $(compgen -W "--message --descriptor-set --proto --include --format --help" -- "$cur") )
            else
                _pbkit_query_paths
            fi
            ;;
        completions)
            COMPREPLY=( $(compgen -W "bash elvish fish powershell zsh" -- "$cur") )
            ;;
        complete)
            case "$prev" in
                --proto) _pbkit_proto_files; return ;;
                -I|--include) compopt -o dirnames 2>/dev/null; return ;;
                --descriptor-set) compopt -o default 2>/dev/null; return ;;
                -m|--message) _pbkit_messages; return ;;
            esac
            if [[ -z "$target" ]]; then
                _pbkit_complete_targets
            elif [[ "$cur" == --* ]]; then
                COMPREPLY=( $(compgen -W "--prefix --message --descriptor-set --proto --include --details --help" -- "$cur") )
            else
                case "$target" in
                    proto-files) _pbkit_proto_files ;;
                    messages) _pbkit_messages ;;
                    fields) _pbkit_fields ;;
                    query-path) _pbkit_query_paths ;;
                esac
            fi
            ;;
        *)
            _pbkit_subcommands
            ;;
    esac
}

complete -F _pbkit pbkit
"#
}

fn enhanced_fish_completion() -> &'static str {
    r#"# Generated by `pbkit completions fish`.
# Install:
#   mkdir -p ~/.config/fish/completions
#   pbkit completions fish > ~/.config/fish/completions/pbkit.fish
#
# This completion calls `pbkit complete ...` for proto-aware candidates.

function __pbkit_schema_args
    set -l tokens (commandline -opc)
    set -l i 1
    while test $i -le (count $tokens)
        set -l word $tokens[$i]
        switch $word
            case --descriptor-set --proto -I --include
                set -l j (math $i + 1)
                if test $j -le (count $tokens)
                    printf '%s\n' $word $tokens[$j]
                end
            case '--descriptor-set=*'
                printf '%s\n' --descriptor-set (string replace -- '--descriptor-set=' '' $word)
            case '--proto=*'
                printf '%s\n' --proto (string replace -- '--proto=' '' $word)
            case '--include=*'
                printf '%s\n' -I (string replace -- '--include=' '' $word)
            case '-I?*'
                printf '%s\n' -I (string sub -s 3 -- $word)
        end
        set i (math $i + 1)
    end
end

function __pbkit_message_arg
    set -l tokens (commandline -opc)
    set -l i 1
    while test $i -le (count $tokens)
        set -l word $tokens[$i]
        switch $word
            case -m --message
                set -l j (math $i + 1)
                if test $j -le (count $tokens)
                    printf '%s\n' $tokens[$j]
                    return
                end
            case '--message=*'
                string replace -- '--message=' '' $word
                return
        end
        set i (math $i + 1)
    end
end

function __pbkit_proto_files
    set -l cmd (commandline -opc)[1]
    set -l prefix (commandline -ct)
    set -l schema_args (__pbkit_schema_args)
    $cmd complete proto-files $schema_args --prefix "$prefix" --details 2>/dev/null
end

function __pbkit_messages
    set -l cmd (commandline -opc)[1]
    set -l prefix (commandline -ct)
    set -l schema_args (__pbkit_schema_args)
    $cmd complete messages $schema_args --prefix "$prefix" --details 2>/dev/null
end

function __pbkit_fields
    set -l cmd (commandline -opc)[1]
    set -l prefix (commandline -ct)
    set -l schema_args (__pbkit_schema_args)
    set -l message (__pbkit_message_arg)
    if test -n "$message"
        $cmd complete fields $schema_args --message "$message" --prefix "$prefix" --details 2>/dev/null
    else
        $cmd complete fields $schema_args --prefix "$prefix" --details 2>/dev/null
    end
end

function __pbkit_query_paths
    set -l cmd (commandline -opc)[1]
    set -l prefix (commandline -ct)
    set -l schema_args (__pbkit_schema_args)
    set -l message (__pbkit_message_arg)
    if test -n "$message"
        $cmd complete query-path $schema_args --message "$message" --prefix "$prefix" --details 2>/dev/null
    else
        $cmd complete query-path $schema_args --prefix "$prefix" --details 2>/dev/null
    end
end

function __pbkit_complete_targets
    printf '%s\n' \
        'proto-files	Proto files from -I/current directory' \
        'messages	Message names from descriptors' \
        'enums	Enum names from descriptors' \
        'fields	Field names for --message' \
        'query-path	JSONPath-like field paths for --message'
end

complete -c pbkit -f
complete -c pbkit -n 'not __fish_seen_subcommand_from sort fmt lint decode query completions complete help' -a 'sort	Sort message/enum declarations and field lines'
complete -c pbkit -n 'not __fish_seen_subcommand_from sort fmt lint decode query completions complete help' -a 'fmt	Format proto files'
complete -c pbkit -n 'not __fish_seen_subcommand_from sort fmt lint decode query completions complete help' -a 'lint	Lint proto files'
complete -c pbkit -n 'not __fish_seen_subcommand_from sort fmt lint decode query completions complete help' -a 'decode	Decode protobuf binary without descriptors'
complete -c pbkit -n 'not __fish_seen_subcommand_from sort fmt lint decode query completions complete help' -a 'query	Query protobuf binary with a JSONPath-like selector'
complete -c pbkit -n 'not __fish_seen_subcommand_from sort fmt lint decode query completions complete help' -a 'completions	Generate shell completion scripts'
complete -c pbkit -n 'not __fish_seen_subcommand_from sort fmt lint decode query completions complete help' -a 'complete	Print proto-aware completion candidates'

complete -c pbkit -n '__fish_seen_subcommand_from sort' -s w -l write -d 'Write sorted output back'
complete -c pbkit -n '__fish_seen_subcommand_from sort' -l fields -r -a 'number name'
complete -c pbkit -n '__fish_seen_subcommand_from sort' -a '(__pbkit_proto_files)'

complete -c pbkit -n '__fish_seen_subcommand_from fmt' -s w -l write -d 'Write formatted output back'
complete -c pbkit -n '__fish_seen_subcommand_from fmt' -l check -d 'Check formatting'
complete -c pbkit -n '__fish_seen_subcommand_from fmt' -l diff -d 'Print unified diff'
complete -c pbkit -n '__fish_seen_subcommand_from fmt' -l config -r -a '(__fish_complete_path)'
complete -c pbkit -n '__fish_seen_subcommand_from fmt' -l recursive -d 'Recurse into directory inputs'
complete -c pbkit -n '__fish_seen_subcommand_from fmt' -l without-sort -d 'Skip sorting'
complete -c pbkit -n '__fish_seen_subcommand_from fmt' -l fields -r -a 'number name'
complete -c pbkit -n '__fish_seen_subcommand_from fmt' -a '(__pbkit_proto_files)'

complete -c pbkit -n '__fish_seen_subcommand_from lint' -l config -r -a '(__fish_complete_path)'
complete -c pbkit -n '__fish_seen_subcommand_from lint' -l recursive -d 'Recurse into directory inputs'
complete -c pbkit -n '__fish_seen_subcommand_from lint' -l without-sort -d 'Skip sort checks'
complete -c pbkit -n '__fish_seen_subcommand_from lint' -a '(__pbkit_proto_files)'

complete -c pbkit -n '__fish_seen_subcommand_from decode' -a '(__fish_complete_path)'

complete -c pbkit -n '__fish_seen_subcommand_from query' -s m -l message -r -a '(__pbkit_messages)'
complete -c pbkit -n '__fish_seen_subcommand_from query' -l descriptor-set -r -a '(__fish_complete_path)'
complete -c pbkit -n '__fish_seen_subcommand_from query' -l proto -r -a '(__pbkit_proto_files)'
complete -c pbkit -n '__fish_seen_subcommand_from query' -s I -l include -r -a '(__fish_complete_directories)'
complete -c pbkit -n '__fish_seen_subcommand_from query' -s f -l format -r -a 'json raw hex base64'
complete -c pbkit -n '__fish_seen_subcommand_from query' -a '(__pbkit_query_paths)'

complete -c pbkit -n '__fish_seen_subcommand_from completions' -a 'bash elvish fish powershell zsh'

complete -c pbkit -n '__fish_seen_subcommand_from complete; and not __fish_seen_subcommand_from proto-files messages enums fields query-path' -a '(__pbkit_complete_targets)'
complete -c pbkit -n '__fish_seen_subcommand_from complete' -l prefix -r
complete -c pbkit -n '__fish_seen_subcommand_from complete' -s m -l message -r -a '(__pbkit_messages)'
complete -c pbkit -n '__fish_seen_subcommand_from complete' -l descriptor-set -r -a '(__fish_complete_path)'
complete -c pbkit -n '__fish_seen_subcommand_from complete' -l proto -r -a '(__pbkit_proto_files)'
complete -c pbkit -n '__fish_seen_subcommand_from complete' -s I -l include -r -a '(__fish_complete_directories)'
complete -c pbkit -n '__fish_seen_subcommand_from complete' -l details
complete -c pbkit -n '__fish_seen_subcommand_from complete; and __fish_seen_subcommand_from fields' -a '(__pbkit_fields)'
complete -c pbkit -n '__fish_seen_subcommand_from complete; and __fish_seen_subcommand_from query-path' -a '(__pbkit_query_paths)'
"#
}

fn enhanced_zsh_completion() -> &'static str {
    r#"#compdef pbkit

# Generated by `pbkit completions zsh`.
# Install:
#   mkdir -p ~/.zfunc
#   pbkit completions zsh > ~/.zfunc/_pbkit
#   echo 'fpath=(~/.zfunc $fpath); autoload -Uz compinit; compinit' >> ~/.zshrc
#
# This completion calls `pbkit complete ...` for proto-aware candidates.

_pbkit_schema_args() {
  reply=()
  local i word next
  for (( i = 1; i < CURRENT; i++ )); do
    word="${words[i]}"
    case "$word" in
      --descriptor-set)
        next="${words[i+1]}"
        [[ -n "$next" && "$next" != -* ]] && reply+=(--descriptor-set "$next")
        ;;
      --descriptor-set=*)
        reply+=(--descriptor-set "${word#--descriptor-set=}")
        ;;
      --proto)
        next="${words[i+1]}"
        [[ -n "$next" && "$next" != -* ]] && reply+=(--proto "$next")
        ;;
      --proto=*)
        reply+=(--proto "${word#--proto=}")
        ;;
      -I|--include)
        next="${words[i+1]}"
        [[ -n "$next" && "$next" != -* ]] && reply+=(-I "$next")
        ;;
      -I?*)
        reply+=(-I "${word#-I}")
        ;;
      --include=*)
        reply+=(-I "${word#--include=}")
        ;;
    esac
  done
}

_pbkit_message_arg() {
  reply=()
  local i word next
  for (( i = 1; i < CURRENT; i++ )); do
    word="${words[i]}"
    case "$word" in
      -m|--message)
        next="${words[i+1]}"
        [[ -n "$next" && "$next" != -* ]] && reply=("$next")
        ;;
      --message=*)
        reply=("${word#--message=}")
        ;;
    esac
  done
}

_pbkit_describe_lines() {
  local tag="$1"
  local label="$2"
  shift 2
  local -a rows matches
  rows=("$@")
  matches=()
  local row value detail
  for row in "${rows[@]}"; do
    value="${row%%	*}"
    if [[ "$row" == *$'\t'* ]]; then
      detail="${row#*	}"
      matches+=("${value}:${detail}")
    else
      matches+=("${value}")
    fi
  done
  (( ${#matches[@]} )) && _describe -t "$tag" "$label" matches
}

_pbkit_proto_files() {
  _pbkit_schema_args
  local -a schema_args rows
  schema_args=("${reply[@]}")
  rows=("${(@f)$(${words[1]} complete proto-files "${schema_args[@]}" --prefix "$PREFIX" --details 2>/dev/null)}")
  if (( ${#rows[@]} )); then
    _pbkit_describe_lines proto-files 'proto files' "${rows[@]}"
  else
    _files -g '*.proto'
  fi
}

_pbkit_messages() {
  _pbkit_schema_args
  local -a schema_args rows
  schema_args=("${reply[@]}")
  rows=("${(@f)$(${words[1]} complete messages "${schema_args[@]}" --prefix "$PREFIX" --details 2>/dev/null)}")
  _pbkit_describe_lines messages 'messages' "${rows[@]}"
}

_pbkit_fields() {
  _pbkit_schema_args
  local -a schema_args message rows
  schema_args=("${reply[@]}")
  _pbkit_message_arg
  message="${reply[1]}"
  if [[ -n "$message" ]]; then
    rows=("${(@f)$(${words[1]} complete fields "${schema_args[@]}" --message "$message" --prefix "$PREFIX" --details 2>/dev/null)}")
  else
    rows=("${(@f)$(${words[1]} complete fields "${schema_args[@]}" --prefix "$PREFIX" --details 2>/dev/null)}")
  fi
  _pbkit_describe_lines fields 'fields' "${rows[@]}"
}

_pbkit_query_paths() {
  _pbkit_schema_args
  local -a schema_args message rows
  schema_args=("${reply[@]}")
  _pbkit_message_arg
  message="${reply[1]}"
  if [[ -n "$message" ]]; then
    rows=("${(@f)$(${words[1]} complete query-path "${schema_args[@]}" --message "$message" --prefix "$PREFIX" --details 2>/dev/null)}")
  else
    rows=("${(@f)$(${words[1]} complete query-path "${schema_args[@]}" --prefix "$PREFIX" --details 2>/dev/null)}")
  fi
  _pbkit_describe_lines query-paths 'query paths' "${rows[@]}"
}

_pbkit_commands() {
  local -a commands
  commands=(
    'sort:Sort message/enum declarations and field lines'
    'fmt:Format proto files'
    'lint:Lint proto files'
    'decode:Decode protobuf binary without descriptors'
    'query:Query protobuf binary with a JSONPath-like selector'
    'completions:Generate shell completion scripts'
    'complete:Print proto-aware completion candidates'
    'help:Print help'
  )
  _describe -t commands 'pbkit commands' commands
}

_pbkit_complete_targets() {
  local -a targets
  targets=(
    'proto-files:Proto files from -I/current directory'
    'messages:Message names from descriptors'
    'enums:Enum names from descriptors'
    'fields:Field names for --message'
    'query-path:JSONPath-like field paths for --message'
  )
  _describe -t complete-targets 'completion target' targets
}

_pbkit_option_or_file() {
  if [[ "$PREFIX" == --* ]]; then
    compadd -- --help
  else
    _files
  fi
}

_pbkit_sort() {
  case "${words[CURRENT-1]}" in
    --fields) compadd -- number name; return ;;
  esac
  if [[ "$PREFIX" == --* ]]; then
    compadd -- --write --fields --help
  else
    _pbkit_proto_files
  fi
}

_pbkit_decode() {
  if [[ "$PREFIX" == --* ]]; then
    compadd -- --help
  else
    _files
  fi
}

_pbkit_fmt() {
  case "${words[CURRENT-1]}" in
    --fields) compadd -- number name; return ;;
    --config) _files; return ;;
  esac
  if [[ "$PREFIX" == --* ]]; then
    compadd -- --write --check --diff --config --recursive --without-sort --fields --help
  else
    _pbkit_proto_files
  fi
}

_pbkit_lint() {
  case "${words[CURRENT-1]}" in
    --config) _files; return ;;
  esac
  if [[ "$PREFIX" == --* ]]; then
    compadd -- --config --recursive --without-sort --help
  else
    _pbkit_proto_files
  fi
}

_pbkit_query() {
  case "${words[CURRENT-1]}" in
    --proto) _pbkit_proto_files; return ;;
    -I|--include) _directories; return ;;
    --descriptor-set) _files; return ;;
    -m|--message) _pbkit_messages; return ;;
    -o|--format) compadd -- json raw hex base64; return ;;
  esac
  if [[ "$PREFIX" == --* ]]; then
    compadd -- --message --descriptor-set --proto --include --format --help
  elif [[ "$PREFIX" == '$'* || "$PREFIX" == .* ]]; then
    _pbkit_query_paths || _files
  else
    _pbkit_query_paths || _files
  fi
}

_pbkit_completions() {
  compadd -- bash elvish fish powershell zsh
}

_pbkit_complete_cmd() {
  local target=""
  local i
  for (( i = 3; i < CURRENT; i++ )); do
    if [[ "${words[i]}" != -* ]]; then
      target="${words[i]}"
      break
    fi
  done

  case "${words[CURRENT-1]}" in
    --proto) _pbkit_proto_files; return ;;
    -I|--include) _directories; return ;;
    --descriptor-set) _files; return ;;
    -m|--message) _pbkit_messages; return ;;
  esac

  if (( CURRENT == 3 )); then
    _pbkit_complete_targets
    return
  fi

  if [[ "$PREFIX" == --* ]]; then
    compadd -- --prefix --message --descriptor-set --proto --include --details --help
  else
    case "$target" in
      proto-files) _pbkit_proto_files ;;
      messages) _pbkit_messages ;;
      enums) compadd -- ;;
      fields) _pbkit_fields ;;
      query-path) _pbkit_query_paths ;;
      *) _pbkit_complete_targets ;;
    esac
  fi
}

_pbkit() {
  if (( CURRENT == 2 )); then
    _pbkit_commands
    return
  fi

  case "${words[2]}" in
    sort) _pbkit_sort ;;
    fmt) _pbkit_fmt ;;
    lint) _pbkit_lint ;;
    decode) _pbkit_decode ;;
    query) _pbkit_query ;;
    completions) _pbkit_completions ;;
    complete) _pbkit_complete_cmd ;;
    help) _pbkit_commands ;;
    *) _pbkit_commands ;;
  esac
}

_pbkit "$@"
"#
}

fn run_complete(
    target: CompleteTarget,
    prefix: &str,
    message: Option<String>,
    descriptor_set: Option<PathBuf>,
    proto_files: Vec<PathBuf>,
    includes: Vec<PathBuf>,
    details: bool,
) -> Result<()> {
    if matches!(target, CompleteTarget::ProtoFiles) {
        print_candidates(&proto_file_candidates(&includes, prefix)?, details);
        return Ok(());
    }

    if descriptor_set.is_none() && proto_files.is_empty() {
        bail!("complete requires --descriptor-set or at least one --proto file");
    }

    let pool = load_pool(descriptor_set.as_deref(), &proto_files, &includes)?;
    let candidates = match target {
        CompleteTarget::ProtoFiles => unreachable!(),
        CompleteTarget::Messages => message_candidates(&pool, prefix),
        CompleteTarget::Enums => enum_candidates(&pool, prefix),
        CompleteTarget::Fields => {
            let message = resolve_message_name(&pool, message, "field completion")?;
            field_candidates(&pool, &message, prefix)?
        }
        CompleteTarget::QueryPath => {
            let message = resolve_message_name(&pool, message, "query-path completion")?;
            query_path_candidates(&pool, &message, prefix)?
        }
    };

    print_candidates(&candidates, details);
    Ok(())
}

fn resolve_message_name(
    pool: &DescriptorPool,
    message: Option<String>,
    context: &str,
) -> Result<String> {
    if let Some(message) = message {
        return Ok(message);
    }

    let messages = pool
        .all_messages()
        .filter(|message| !message.is_map_entry())
        .map(|message| message.full_name().to_owned())
        .collect::<Vec<_>>();
    match messages.as_slice() {
        [message] => Ok(message.clone()),
        [] => bail!("--message is required for {context}; no message types were found"),
        _ => bail!("--message is required for {context}; found multiple message types"),
    }
}

fn proto_file_candidates(includes: &[PathBuf], prefix: &str) -> Result<Vec<Candidate>> {
    let mut roots = if includes.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        includes.to_vec()
    };
    if !roots.iter().any(|root| root == &PathBuf::from(".")) {
        roots.push(PathBuf::from("."));
    }

    let mut candidates = Vec::new();
    for root in roots {
        collect_proto_file_candidates(&root, &root, prefix, &mut candidates)?;
    }
    candidates.sort_by(|a, b| a.value.cmp(&b.value));
    candidates.dedup_by(|a, b| a.value == b.value);
    Ok(candidates)
}

fn collect_proto_file_candidates(
    root: &std::path::Path,
    dir: &std::path::Path,
    prefix: &str,
    candidates: &mut Vec<Candidate>,
) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_proto_file_candidates(root, &path, prefix, candidates)?;
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("proto") {
            continue;
        }
        let value = path.display().to_string();
        let relative = path
            .strip_prefix(root)
            .ok()
            .map(|path| path.display().to_string());
        let file_name = path.file_name().and_then(|name| name.to_str());
        if value.starts_with(prefix)
            || relative
                .as_deref()
                .is_some_and(|relative| relative.starts_with(prefix))
            || file_name.is_some_and(|name| name.starts_with(prefix))
        {
            candidates.push(Candidate {
                value,
                detail: "proto".into(),
            });
        }
    }
    Ok(())
}

fn print_candidates(candidates: &[Candidate], details: bool) {
    for candidate in candidates {
        if details {
            println!("{}\t{}", candidate.value, candidate.detail);
        } else {
            println!("{}", candidate.value);
        }
    }
}

fn read_input(input: Option<&PathBuf>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    match input {
        Some(path) => {
            bytes = std::fs::read(path)
                .with_context(|| format!("failed to read input {}", path.display()))?;
        }
        None => {
            io::stdin()
                .read_to_end(&mut bytes)
                .context("failed to read stdin")?;
        }
    }
    Ok(bytes)
}

fn write_value(value: &Value, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(value),
        OutputFormat::Raw => {
            let bytes = raw_bytes_from_json(value)
                .context("--format raw only works for raw length-delimited wire values")?;
            io::stdout()
                .write_all(&bytes)
                .context("failed to write raw output")
        }
        OutputFormat::Hex => {
            let bytes = raw_bytes_from_json(value)
                .context("--format hex only works for raw length-delimited wire values")?;
            println!("{}", hex::encode(bytes));
            Ok(())
        }
        OutputFormat::Base64 => {
            if let Some(s) = value.get("bytes_base64").and_then(Value::as_str) {
                println!("{s}");
                return Ok(());
            }
            let bytes = raw_bytes_from_json(value)
                .context("--format base64 only works for raw length-delimited wire values")?;
            use base64::Engine;
            println!(
                "{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            );
            Ok(())
        }
    }
}

fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

impl From<FieldSort> for SortKey {
    fn from(value: FieldSort) -> Self {
        match value {
            FieldSort::Number => SortKey::Number,
            FieldSort::Name => SortKey::Name,
        }
    }
}
