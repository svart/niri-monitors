use clap::Parser;
use niri_monitors::config::{default_config_path, load_config_or_empty};
use niri_monitors::control::{default_control_socket_path, start_control_socket};
use niri_monitors::daemon::{DaemonState, reconcile_once, run_loop_shared};
use niri_monitors::niri::ipc::SocketNiriClient;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "niri-monitorsd",
    about = "Automatically apply niri monitor profiles"
)]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    once: bool,
    #[arg(long, default_value = "info")]
    log_level: String,
    #[arg(long)]
    no_control_socket: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    init_logging(&args.log_level)?;

    let config_path = args.config.map_or_else(default_config_path, Ok)?;
    let config = load_config_or_empty(&config_path)?;
    let state = Arc::new(Mutex::new(DaemonState::new(config)));
    let mut client = SocketNiriClient::from_env()?;

    if args.once {
        let mut state = state.lock().map_err(|_| "daemon state lock poisoned")?;
        let outcome = reconcile_once(&mut client, &mut state, args.dry_run)?;
        if let Some(dry_run_output) = outcome.dry_run_output {
            println!("{dry_run_output}");
        }
        return Ok(());
    }

    if !args.no_control_socket {
        let socket_path = default_control_socket_path()?;
        let _control_thread = start_control_socket(socket_path, state.clone(), config_path)?;
    } else {
        tracing::debug!("control socket disabled");
    }

    run_loop_shared(&mut client, state, args.dry_run)?;
    Ok(())
}

fn init_logging(log_level: &str) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(log_level)?)
        .init();
    Ok(())
}
