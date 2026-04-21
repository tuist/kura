use std::{collections::BTreeSet, sync::Arc, time::Duration};

use axum::response::Response;
use http_body_util::BodyExt;
use reqwest::Client;
use tempfile::TempDir;
use tokio::sync::{Notify, RwLock};

use crate::{config::Config, metrics::Metrics, state::AppState, store::Store};

pub(crate) struct TestContext {
    pub _temp_dir: TempDir,
    pub state: Arc<AppState>,
}

pub(crate) async fn test_context<F>(override_config: F) -> TestContext
where
    F: FnOnce(&mut Config),
{
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let mut config = Config {
        port: 0,
        tenant_id: "test-tenant".into(),
        region: "local".into(),
        tmp_dir: temp_dir.path().join("tmp"),
        data_dir: temp_dir.path().join("data"),
        node_url: "http://127.0.0.1:0".into(),
        peers: vec!["http://127.0.0.1:0".into()],
        otlp_traces_endpoint: "http://127.0.0.1:4318/v1/traces".into(),
        otel_service_name: "kura-test".into(),
        otel_deployment_environment: "test".into(),
    };
    override_config(&mut config);
    config
        .ensure_directories()
        .await
        .expect("failed to create test directories");

    let store = Store::open(&config).expect("failed to open test store");
    let metrics = Metrics::new(config.region.clone(), config.tenant_id.clone());
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("failed to build test client");
    let state = Arc::new(AppState {
        config,
        store,
        metrics,
        client,
        notify: Notify::new(),
        members: RwLock::new(BTreeSet::new()),
    });

    TestContext {
        _temp_dir: temp_dir,
        state,
    }
}

pub(crate) async fn response_text(response: Response) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("failed to collect response body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("response body should be utf-8")
}
