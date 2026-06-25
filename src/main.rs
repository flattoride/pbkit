use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum, ValueHint};
use clap_complete::Shell;
use pbkit::{
    lint::{LintDiagnostic, LintOptions, Severity, lint_proto},
    path::{parse_path, select},
    proto_fmt::{FormatOptions, format_proto},
    proto_sort::{SortKey, sort_proto},
    reflect::{decode_to_json, load_pool},
    schema::{
        Candidate, enum_candidates, field_candidates, message_candidates, query_path_candidates,
    },
    wire::{decode_message, raw_bytes_from_json, to_json},
};
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
    Fmt {
        /// Proto files to format. Reads stdin when omitted.
        #[arg(value_hint = ValueHint::FilePath)]
        files: Vec<PathBuf>,
        /// Write formatted output back to each input file.
        #[arg(short, long)]
        write: bool,
        /// Exit with a non-zero status when formatting would change output.
        #[arg(long)]
        check: bool,
        /// Format without moving imports, declarations, or fields.
        #[arg(long)]
        without_sort: bool,
        /// Also sort message, enum, service, and extend declarations.
        #[arg(long)]
        sort_declarations: bool,
        /// Sort fields by tag number or field name.
        #[arg(long, default_value = "number")]
        fields: FieldSort,
    },
    /// Lint proto files. By default this includes pbkit fmt/sort checks.
    Lint {
        /// Proto files to lint.
        #[arg(required = true, value_hint = ValueHint::FilePath)]
        files: Vec<PathBuf>,
        /// Skip sort/order checks.
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
    Query {
        /// Path such as '$.items[0].id' for descriptors or '$.2[0].message.1[0]' for raw wire data.
        path: String,
        /// Optional protobuf binary input. Reads stdin when omitted.
        #[arg(value_hint = ValueHint::FilePath)]
        input: Option<PathBuf>,
        /// Fully-qualified message name used with --descriptor-set or --proto.
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
        /// Fully-qualified message name used for fields or query-path.
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
            without_sort,
            sort_declarations,
            fields,
        } => run_fmt(
            files,
            write,
            check,
            FormatOptions {
                sort_imports: !without_sort,
                sort_fields: !without_sort,
                sort_declarations: !without_sort && sort_declarations,
                field_key: fields.into(),
            },
        ),
        Command::Lint {
            files,
            without_sort,
        } => run_lint(
            files,
            LintOptions {
                check_sort: !without_sort,
            },
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

fn run_fmt(files: Vec<PathBuf>, write: bool, check: bool, options: FormatOptions) -> Result<()> {
    if files.is_empty() {
        if write || check {
            bail!("--write and --check require at least one input file");
        }
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("failed to read stdin")?;
        print!("{}", format_proto(&source, options)?);
        return Ok(());
    }

    let mut changed = false;
    for file in files {
        let source = std::fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        let formatted = format_proto(&source, options)
            .with_context(|| format!("failed to format {}", file.display()))?;
        if formatted != source {
            changed = true;
        }
        if write {
            std::fs::write(&file, formatted)
                .with_context(|| format!("failed to write {}", file.display()))?;
        } else if !check {
            print!("{formatted}");
        }
    }

    if check && changed {
        bail!("format check failed");
    }
    Ok(())
}

fn run_lint(files: Vec<PathBuf>, options: LintOptions) -> Result<()> {
    let mut failed = false;
    for file in files {
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
        let message = message.context("--message is required with --descriptor-set or --proto")?;
        let pool = load_pool(descriptor_set.as_deref(), &proto_files, &includes)?;
        decode_to_json(&pool, &message, &bytes)?
    } else {
        to_json(&decode_message(&bytes)?)
    };

    let path = parse_path(query)?;
    let selected = select(&root, &path).with_context(|| format!("path {query:?} did not match"))?;
    write_value(selected, format)
}

fn run_completions(shell: Shell) -> Result<()> {
    if shell == Shell::Zsh {
        print!("{}", enhanced_zsh_completion());
        return Ok(());
    }

    let mut command = Cli::command();
    clap_complete::generate(shell, &mut command, "pbkit", &mut io::stdout());
    Ok(())
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
  [[ -z "$message" ]] && return 1
  rows=("${(@f)$(${words[1]} complete fields "${schema_args[@]}" --message "$message" --prefix "$PREFIX" --details 2>/dev/null)}")
  _pbkit_describe_lines fields 'fields' "${rows[@]}"
}

_pbkit_query_paths() {
  _pbkit_schema_args
  local -a schema_args message rows
  schema_args=("${reply[@]}")
  _pbkit_message_arg
  message="${reply[1]}"
  [[ -z "$message" ]] && return 1
  rows=("${(@f)$(${words[1]} complete query-path "${schema_args[@]}" --message "$message" --prefix "$PREFIX" --details 2>/dev/null)}")
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
  esac
  if [[ "$PREFIX" == --* ]]; then
    compadd -- --write --check --without-sort --sort-declarations --fields --help
  else
    _pbkit_proto_files
  fi
}

_pbkit_lint() {
  if [[ "$PREFIX" == --* ]]; then
    compadd -- --without-sort --help
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
            let message = message.context("--message is required for field completion")?;
            field_candidates(&pool, &message, prefix)?
        }
        CompleteTarget::QueryPath => {
            let message = message.context("--message is required for query-path completion")?;
            query_path_candidates(&pool, &message, prefix)?
        }
    };

    print_candidates(&candidates, details);
    Ok(())
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
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("proto") {
                continue;
            }
            let value = path.display().to_string();
            if value.starts_with(prefix)
                || path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix))
            {
                candidates.push(Candidate {
                    value,
                    detail: "proto".into(),
                });
            }
        }
    }
    candidates.sort_by(|a, b| a.value.cmp(&b.value));
    candidates.dedup_by(|a, b| a.value == b.value);
    Ok(candidates)
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
