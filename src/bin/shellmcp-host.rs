#![cfg_attr(windows, windows_subsystem = "windows")]
// A GUI-subsystem entry point: scheduled tasks never allocate a console.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    let mut log = None;
    if let Some(index) = args.iter().position(|v| v == "--data-dir")
        && let Some(dir) = args.get(index + 1)
    {
        let path = std::path::Path::new(dir).join("host.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_ansi(false)
            .with_writer(file.try_clone()?)
            .try_init()
            .ok();
        log = Some(file);
    }
    let result = shellmcp_agent::entry::run().await;
    if let (Err(error), Some(file)) = (&result, log.as_mut()) {
        use std::io::Write;
        let _ = writeln!(file, "host error: {error:#}");
    }
    result
}
