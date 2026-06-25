use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use pbkit::{
    path::{parse_path, select},
    proto_sort::{SortKey, sort_proto},
    reflect::{decode_to_json, load_pool},
    wire::{decode_message, raw_bytes_from_json, to_json},
};
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "A pragmatic protobuf sorting and binary query toolkit."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Sort message/enum declarations and field lines in .proto text.
    Sort {
        /// Proto files to sort. Reads stdin when omitted.
        files: Vec<PathBuf>,
        /// Write sorted output back to each input file.
        #[arg(short, long)]
        write: bool,
        /// Sort fields by tag number or field name.
        #[arg(long, default_value = "number")]
        fields: FieldSort,
    },
    /// Decode a protobuf binary without descriptors into numbered wire fields.
    Decode {
        /// Optional protobuf binary input. Reads stdin when omitted.
        input: Option<PathBuf>,
    },
    /// Query a protobuf binary using a small JSONPath-like selector.
    Query {
        /// Path such as '$.items[0].id' for descriptors or '$.2[0].message.1[0]' for raw wire data.
        path: String,
        /// Optional protobuf binary input. Reads stdin when omitted.
        input: Option<PathBuf>,
        /// Fully-qualified message name used with --descriptor-set or --proto.
        #[arg(short, long)]
        message: Option<String>,
        /// Binary FileDescriptorSet produced by protoc --descriptor_set_out.
        #[arg(long)]
        descriptor_set: Option<PathBuf>,
        /// Proto source file. Can be repeated.
        #[arg(long = "proto")]
        proto_files: Vec<PathBuf>,
        /// Proto include path. Can be repeated. Defaults to current directory.
        #[arg(short = 'I', long = "include")]
        includes: Vec<PathBuf>,
        /// Output selected data as json, raw bytes, hex, or base64.
        #[arg(short, long, default_value = "json")]
        format: OutputFormat,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sort {
            files,
            write,
            fields,
        } => run_sort(files, write, fields.into()),
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
