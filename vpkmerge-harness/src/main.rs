use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::PathBuf;
use vpkmerge_harness::{execute, init_project, Project, ToolCall};

#[derive(Parser)]
#[command(
    version,
    about = "Local modding harness: project setup and structured read-only tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create mod-project.json, refusing to overwrite an existing project.
    Init {
        directory: PathBuf,
        #[arg(long)]
        name: String,
    },
    /// Report configured and implemented capabilities as JSON.
    Doctor { project: PathBuf },
    /// Execute one JSON `ToolCall` read from stdin. This is not an MCP transport.
    Call { project: PathBuf },
}

fn run() -> Result<()> {
    let output = match Cli::parse().command {
        Command::Init { directory, name } => json!({"project": init_project(&directory, name)?}),
        Command::Doctor { project } => {
            execute(&Project::load(&project)?, ToolCall::GetCapabilities {})?
        }
        Command::Call { project } => {
            let call = serde_json::from_reader(std::io::stdin().lock())?;
            execute(&Project::load(&project)?, call)?
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"ok": true, "result": output}))?
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        println!("{}", json!({"ok": false, "error": format!("{error:#}")}));
        std::process::exit(1);
    }
}
