//! Command-line interface that talks to `aifs-engine` over stdio.

use aifs_engine_client::{discover_engine_binary, EngineClient};
use aifs_protocol::ScanOptions;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

/// AI File Sorter CLI. Every command is executed by the isolated engine process.
#[derive(Debug, Parser)]
#[command(name = "aifs", version, about)]
struct Cli {
    /// Path to `aifs-engine`. Defaults to `$AIFS_ENGINE` or a sibling binary.
    #[arg(long, global = true)]
    engine: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Scan a folder and print a summary (or JSON with `--json`).
    Scan {
        /// Folder to scan.
        folder: PathBuf,
        /// Dump the full workspace snapshot as JSON.
        #[arg(long)]
        json: bool,
        /// Include hidden files.
        #[arg(long)]
        include_hidden: bool,
        /// Do not recurse into subfolders.
        #[arg(long)]
        no_recursive: bool,
        /// Traverse into recognised project trees.
        #[arg(long)]
        no_protect_projects: bool,
        /// Skip media-tag extraction.
        #[arg(long)]
        no_extract: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let engine_path = match cli.engine {
        Some(path) => path,
        None => discover_engine_binary()?,
    };
    match cli.command {
        Commands::Scan {
            folder,
            json,
            include_hidden,
            no_recursive,
            no_protect_projects,
            no_extract,
        } => {
            let mut client = EngineClient::connect(&engine_path, "aifs-cli")?;
            let options = ScanOptions {
                recursive: !no_recursive,
                include_hidden,
                protect_projects: !no_protect_projects,
                extract_metadata: !no_extract,
                ..ScanOptions::default()
            };
            let snapshot = client.scan(&folder, options, None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                println!("Scanned {}", snapshot.root.display());
                println!(
                    "  {} entries  {} skipped  {} projects  {} bundles  {} evidence",
                    snapshot.entries.len(),
                    snapshot.skipped.len(),
                    snapshot.projects.len(),
                    snapshot.bundles.len(),
                    snapshot.evidence.len()
                );
                println!("  session {}", snapshot.session);
                for project in &snapshot.projects {
                    println!(
                        "  project {} ({}) at {}",
                        project.name, project.rule_id, project.root
                    );
                }
            }
            let _ = client.shutdown();
            Ok(())
        }
    }
}
