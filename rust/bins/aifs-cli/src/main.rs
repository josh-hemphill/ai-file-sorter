//! Command-line interface that talks to `aifs-engine` over stdio.

use aifs_domain::{JournalStatus, RevisionAuthor, RevisionPatch};
use aifs_engine_client::{discover_engine_binary, EngineClient};
use aifs_protocol::{ProposalPolicy, ScanOptions};
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
    /// Scan, propose, accept, plan, and apply (default: dry run).
    Organize {
        /// Folder to organise.
        folder: PathBuf,
        /// Dump the plan/journal as JSON.
        #[arg(long)]
        json: bool,
        /// Actually move files. Default is a dry run.
        #[arg(long)]
        apply: bool,
        /// Include hidden files.
        #[arg(long)]
        include_hidden: bool,
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
        Commands::Organize {
            folder,
            json,
            apply,
            include_hidden,
        } => {
            let mut client = EngineClient::connect(&engine_path, "aifs-cli")?;
            let snapshot = client.scan(
                &folder,
                ScanOptions {
                    include_hidden,
                    ..ScanOptions::default()
                },
                None,
            )?;
            let revision = client.propose(snapshot.session, ProposalPolicy::default())?;
            let assets: Vec<_> = revision.placements.keys().copied().collect();
            let accepted = client.patch(
                snapshot.session,
                revision.id,
                RevisionAuthor::User,
                "accept all",
                vec![RevisionPatch::Accept { assets }],
            )?;
            let (plan, issues) = client.plan(snapshot.session, accepted.id)?;
            if issues
                .iter()
                .any(|issue| matches!(issue.severity, aifs_domain::PlanIssueSeverity::Error))
            {
                return Err(format!("plan rejected: {issues:?}").into());
            }
            let journal = client.apply(snapshot.session, plan.id, !apply)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&journal)?);
            }
            if journal.status == JournalStatus::Failed {
                return Err(format!(
                    "apply failed ({} done, {} failed)",
                    journal.done_count(),
                    journal.failed_count()
                )
                .into());
            }
            if !json {
                println!(
                    "{} {} ({} moves, {} done, dry_run={})",
                    if apply { "Applied" } else { "Previewed" },
                    folder.display(),
                    plan.move_count(),
                    journal.done_count(),
                    journal.dry_run
                );
                println!("  session {}  journal {}", snapshot.session, journal.id);
            }
            let _ = client.shutdown();
            Ok(())
        }
    }
}
