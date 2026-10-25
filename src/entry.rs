use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Independent local MCP server and OpenAI tunnel installation.
    Local {
        #[command(subcommand)]
        command: crate::local::cli::Action,
    },

    #[command(hide = true)]
    LocalJobSupervisor {
        #[arg(long)]
        job_id: String,
        #[arg(long)]
        data_dir: std::path::PathBuf,
    },

    SelfCheck,
}

pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    match Cli::parse().command {
        Action::Local { command } => crate::local::cli::run(command).await,

        Action::LocalJobSupervisor { job_id, data_dir } => {
            crate::local::admission::supervise(&data_dir, &job_id)
        }

        Action::SelfCheck => {
            println!("shellmcp-agent {} self-check OK", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
