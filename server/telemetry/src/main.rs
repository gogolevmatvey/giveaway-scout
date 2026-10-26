use giveaway_scout_telemetry::{Error, admin, serve, store::Store};
use std::{path::PathBuf, sync::Arc};
use tokio::net::TcpListener;

async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = terminate.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

async fn run() -> Result<(), Error> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {}
        [argument] if argument == "--version" => {
            println!("giveaway-scout-telemetry {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        [argument] if argument == "verify-live" || argument == "cleanup-client-probe" => {
            let result = if argument == "verify-live" {
                admin::verify_live().await?
            } else {
                admin::cleanup_client_probe().await?
            };
            println!("{}", serde_json::to_string(&result)?);
            return Ok(());
        }
        _ => return Err("Unknown telemetry command".into()),
    }
    let credential = PathBuf::from(
        std::env::var_os("CREDENTIALS_DIRECTORY").ok_or("Missing credentials directory")?,
    )
    .join("installation-salt");
    let database = std::env::var_os("TELEMETRY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/giveaway-scout-telemetry/usage.sqlite3"));
    let store = Arc::new(Store::open(&database, std::fs::read(credential)?)?);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 8793)).await?;
    serve(listener, store, shutdown()).await
}

fn main() -> std::process::ExitCode {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
        .expect("create telemetry runtime");
    if runtime.block_on(run()).is_err() {
        eprintln!("Unable to run telemetry collector");
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
