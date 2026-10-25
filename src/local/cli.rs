use anyhow::Result;
use clap::Subcommand;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Subcommand)]
pub enum Action {
    Install(super::install::Options),
    Uninstall {
        #[arg(long, default_value_os_t=super::config::default_dir())]
        data_dir: PathBuf,
        #[arg(long)]
        purge: bool,
    },
    Recover {
        #[arg(long, default_value_os_t=super::config::default_dir())]
        data_dir: PathBuf,
    },
    #[command(hide = true)]
    TunnelRun {
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        release_dir: PathBuf,
    },
    Init {
        #[arg(long, default_value_os_t=super::config::default_dir())]
        data_dir: PathBuf,
        #[arg(long, default_value = "127.0.0.1:18700")]
        listen: SocketAddr,
    },
    Run {
        #[arg(long, default_value_os_t=super::config::default_dir())]
        data_dir: PathBuf,
        #[arg(long, hide = true)]
        release_dir: Option<PathBuf>,
    },
    Doctor {
        #[arg(long, default_value_os_t=super::config::default_dir())]
        data_dir: PathBuf,
    },
    Policy {
        #[arg(long, default_value_os_t=super::config::default_dir())]
        data_dir: PathBuf,
        #[arg(long)]
        mode: Option<String>,
    },
    Audit {
        #[arg(long, default_value_os_t=super::config::default_dir())]
        data_dir: PathBuf,
        #[arg(long, default_value_t = 20)]
        tail: usize,
    },
}
pub async fn run(action: Action) -> Result<()> {
    match action {
        Action::Install(options) => super::install::install(options).await?,
        Action::Uninstall { data_dir, purge } => {
            super::install::uninstall(&data_dir, purge).await?
        }
        Action::Recover { data_dir } => super::install::recover(&data_dir).await?,
        Action::TunnelRun {
            data_dir,
            release_dir,
        } => super::tunnel::run(&data_dir, &release_dir).await?,
        Action::Init { data_dir, listen } => println!(
            "{}",
            serde_json::to_string_pretty(&super::config::init(&data_dir, listen)?)?
        ),
        Action::Run {
            data_dir,
            release_dir,
        } => super::server::run(&data_dir, release_dir.as_deref()).await?,
        Action::Doctor { data_dir } => println!("{}", super::diagnostics::doctor(&data_dir).await?),
        Action::Policy { data_dir, mode } => {
            super::config::load(&data_dir)?;
            let mut policy = crate::policy::load_checked(&data_dir)?;
            if let Some(mode) = mode {
                policy.mode = serde_json::from_value(serde_json::json!(mode))?;
                crate::policy::save(&data_dir, &policy)?;
            }
            println!("{}", serde_json::to_string_pretty(&policy)?);
        }
        Action::Audit { data_dir, tail } => {
            super::config::load(&data_dir)?;
            for line in crate::policy::tail_audit(&data_dir, tail)? {
                println!("{line}");
            }
        }
    }
    Ok(())
}
