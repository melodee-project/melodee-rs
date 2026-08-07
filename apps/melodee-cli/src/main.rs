use std::{env, process::Command};

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "melodee", version, about = "Melodee Rust administrative CLI bootstrap")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Print repository phase and specification locations.
    Plan,
    /// Check locally available prerequisites without changing the system.
    Doctor,
}

fn command_available(command: &str, version_argument: &str) -> bool {
    Command::new(command)
        .arg(version_argument)
        .output()
        .is_ok_and(|result| result.status.success())
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Plan => {
            println!("Current phase: Phase 0 — Baseline freeze and compatibility capture");
            println!("Read: PRD.md, SPEC.md, IMPLEMENTATION.md, AGENTS.md");
            println!("First work package: design/agents/PHASE-0-WORK-PACKAGE.md");
        }
        Commands::Doctor => {
            let checks = [
                ("ffmpeg", command_available("ffmpeg", "-version")),
                ("ffprobe", command_available("ffprobe", "-version")),
                ("fpcalc", command_available("fpcalc", "-version")),
                ("DATABASE_URL", env::var_os("DATABASE_URL").is_some()),
            ];

            for (name, available) in checks {
                let status = if available { "ok" } else { "missing" };
                println!("{name}: {status}");
            }
        }
    }
}
