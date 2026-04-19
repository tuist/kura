use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use reqwest::Client;
use tokio::sync::{Notify, RwLock};
use tracing::info;

use crate::{
    config::Config,
    http,
    metrics::Metrics,
    replication::{spawn_membership_task, spawn_outbox_task},
    state::AppState,
    store::Store,
    telemetry::init_tracing,
};

pub async fn run() -> Result<(), String> {
    let config = Arc::new(Config::from_env());
    let tracer_provider = init_tracing(config.as_ref());

    config
        .ensure_directories()
        .await
        .map_err(|error| format!("failed to create directories: {error}"))?;

    let store = Arc::new(Store::open(config.clone())?);
    let metrics = Arc::new(Metrics::new(config.region.clone(), config.account.clone()));
    let members = Arc::new(RwLock::new(BTreeSet::new()));
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))?;
    let notify = Arc::new(Notify::new());

    let state = Arc::new(AppState {
        config,
        store,
        metrics,
        client,
        notify,
        members,
    });

    spawn_membership_task(state.clone());
    spawn_outbox_task(state.clone());

    let router = http::router(state.clone());
    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, state.config.port));
    info!("cache Rust service listening on {address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| format!("failed to bind TCP listener: {error}"))?;

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| format!("server error: {error}"))?;

    if let Some(provider) = tracer_provider {
        if let Err(error) = provider.shutdown() {
            eprintln!("failed to shutdown OTLP tracer provider: {error}");
        }
    }

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        signal.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
