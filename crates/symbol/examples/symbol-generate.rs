use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

#[allow(dead_code, unused_imports)]
#[path = "../generation/mod.rs"]
mod generation;

use generation::{GenerationMode, prepare_ledger};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("symbol-generate: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let command = arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
        .ok_or("usage: symbol-generate report [ROOT]; use ./api-version to update or bump")?;
    let root = arguments
        .next()
        .map(PathBuf::from)
        .map_or_else(env::current_dir, Ok)?;
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }

    let ledger = match command.as_str() {
        "report" => prepare_ledger(&root, GenerationMode::ReadOnly)?,
        _ => return Err(format!("unknown command `{command}`").into()),
    };
    println!(
        "{} revision {} {}",
        ledger.version, ledger.absolute_revision, ledger.source_hash
    );
    Ok(())
}
