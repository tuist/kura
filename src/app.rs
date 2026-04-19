use std::{
    collections::BTreeSet,
    future::Future,
    net::{Ipv4Addr, SocketAddr},
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
    let config = Config::from_env();
    let tracer_provider = init_tracing(&config);

    config
        .ensure_directories()
        .await
        .map_err(|error| format!("failed to create directories: {error}"))?;

    let store = Store::open(&config)?;
    let metrics = Metrics::new(config.region.clone(), config.account.clone());
    let members = RwLock::new(BTreeSet::new());
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))?;
    let notify = Notify::new();

    let state = std::sync::Arc::new(AppState {
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

    wait_for_shutdown_signal(ctrl_c, terminate).await;
}

async fn wait_for_shutdown_signal<C, T>(ctrl_c: C, terminate: T)
where
    C: Future<Output = ()>,
    T: Future<Output = ()>,
{
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use tokio::{sync::oneshot, time::timeout};

    use super::*;

    #[tokio::test]
    async fn wait_for_shutdown_signal_returns_when_ctrl_c_resolves() {
        let (ctrl_c_tx, ctrl_c_rx) = oneshot::channel::<()>();
        let (_terminate_tx, terminate_rx) = oneshot::channel::<()>();

        let waiter = tokio::spawn(wait_for_shutdown_signal(
            async move {
                let _ = ctrl_c_rx.await;
            },
            async move {
                let _ = terminate_rx.await;
            },
        ));

        ctrl_c_tx.send(()).expect("ctrl-c sender should be open");

        timeout(Duration::from_secs(1), waiter)
            .await
            .expect("shutdown waiter should return after ctrl-c")
            .expect("shutdown waiter task should finish cleanly");
    }

    #[tokio::test]
    async fn wait_for_shutdown_signal_returns_when_terminate_resolves() {
        let (_ctrl_c_tx, ctrl_c_rx) = oneshot::channel::<()>();
        let (terminate_tx, terminate_rx) = oneshot::channel::<()>();

        let waiter = tokio::spawn(wait_for_shutdown_signal(
            async move {
                let _ = ctrl_c_rx.await;
            },
            async move {
                let _ = terminate_rx.await;
            },
        ));

        terminate_tx
            .send(())
            .expect("terminate sender should be open");

        timeout(Duration::from_secs(1), waiter)
            .await
            .expect("shutdown waiter should return after terminate")
            .expect("shutdown waiter task should finish cleanly");
    }
}
