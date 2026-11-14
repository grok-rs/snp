//! `file-identify`: print identify tags of a path as a JSON array.
//!
//! Mirrors Python's `identify-cli`: `file-identify [--filename-only] PATH`
//! prints e.g. `["file", "non-executable", "python", "text"]` and exits 0,
//! or exits 1 when the path does not exist (message on stderr) or no tag
//! was found.

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use file_identify::{TagSet, tags_from_filename, tags_from_path};

#[derive(Parser)]
#[command(name = "file-identify")]
#[command(
    about = "File identification tool - determines file types based on extensions, content, and shebangs"
)]
#[command(version)]
struct Args {
    /// Only use filename for identification (don't read file contents)
    #[arg(long)]
    filename_only: bool,

    /// Path to the file to identify
    path: PathBuf,
}

/// `json.dumps(sorted(tags))`.
fn tags_to_json(tags: TagSet) -> String {
    let mut out = String::from("[");
    for (i, name) in tags.names().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push('"');
        for c in name.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if c.is_control() => {
                    let _ = write!(out, "\\u{:04x}", u32::from(c));
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }
    out.push(']');
    out
}

fn main() -> ExitCode {
    let args = Args::parse();

    let tags = if args.filename_only {
        tags_from_filename(args.path.as_os_str().as_encoded_bytes())
    } else {
        match tags_from_path(&args.path) {
            Ok(tags) => tags,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
    };

    if tags.is_empty() {
        return ExitCode::FAILURE;
    }
    println!("{}", tags_to_json(tags));
    ExitCode::SUCCESS
}
