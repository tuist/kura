pub mod operation;
pub mod outbox_message;

use std::{collections::BTreeSet, time::Duration};

use reqwest::header::{CONTENT_TYPE, HeaderValue};
use tokio::time::sleep;
use tokio_util::io::ReaderStream;
use tracing::{Instrument, field, warn};

use crate::{
    artifact::manifest::ArtifactManifest,
    constants::REPLICATION_RETRY_SECS,
    state::SharedState,
    telemetry::inject_current_trace_context,
    utils::{replication_target_label, url_encode},
};

use self::{operation::ReplicationOperation, outbox_message::OutboxMessage};

pub fn enqueue_replication_for_artifact(state: &SharedState, manifest: &ArtifactManifest) {
    for peer in state
        .config
        .peers
        .iter()
        .filter(|peer| *peer != &state.config.node_url)
    {
        if let Err(error) = state.store.enqueue(OutboxMessage {
            target: peer.clone(),
            operation: ReplicationOperation::UpsertArtifact {
                kind: manifest.kind,
                namespace_id: manifest.namespace_id.clone(),
                key: manifest.key.clone(),
                content_type: manifest.content_type.clone(),
                artifact_id: manifest.artifact_id.clone(),
                version_ms: manifest.version_ms,
            },
        }) {
            warn!("failed to enqueue artifact replication for {peer}: {error}");
        }
    }
    state.notify.notify_one();
}

pub fn spawn_membership_task(state: SharedState) {
    tokio::spawn(async move {
        loop {
            let mut members = BTreeSet::new();
            for peer in state
                .config
                .peers
                .iter()
                .filter(|peer| *peer != &state.config.node_url)
            {
                match state
                    .client
                    .get(format!("{peer}/_internal/status"))
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => match response
                        .json::<serde_json::Value>()
                        .await
                    {
                        Ok(payload) => {
                            if let Some(region) =
                                payload.get("region").and_then(|value| value.as_str())
                            {
                                members.insert(region.to_owned());
                            }
                        }
                        Err(error) => warn!("failed to decode peer status from {peer}: {error}"),
                    },
                    Ok(response) => {
                        warn!("peer status check failed for {peer}: {}", response.status())
                    }
                    Err(error) => warn!("peer status request failed for {peer}: {error}"),
                }
            }

            *state.members.write().await = members;
            sleep(Duration::from_secs(2)).await;
        }
    });
}

pub fn spawn_outbox_task(state: SharedState) {
    tokio::spawn(async move {
        loop {
            let pause_outbox = state.memory.pause_outbox();
            state
                .metrics
                .update_background_work_paused("outbox", pause_outbox);
            if !pause_outbox {
                if let Err(error) = process_outbox(&state).await {
                    warn!("outbox processing failed: {error}");
                }
            }

            tokio::select! {
                _ = state.notify.notified() => {},
                _ = sleep(Duration::from_secs(REPLICATION_RETRY_SECS)) => {},
            }
        }
    });
}

pub async fn process_outbox(state: &SharedState) -> Result<(), String> {
    for (message_key, message) in state.store.outbox_messages()? {
        let started_at = std::time::Instant::now();
        let operation_name = message.operation.name();
        let result = replicate_message(state, &message).await;

        match result {
            Ok(()) => {
                state.metrics.record_replication(
                    &message.target,
                    operation_name,
                    "ok",
                    started_at.elapsed(),
                );
                state.store.delete_outbox_message(&message_key)?;
            }
            Err(error) => {
                state.metrics.record_replication(
                    &message.target,
                    operation_name,
                    "error",
                    started_at.elapsed(),
                );
                warn!("replication to {} failed: {error}", message.target);
            }
        }
    }

    Ok(())
}

async fn replicate_message(state: &SharedState, message: &OutboxMessage) -> Result<(), String> {
    match &message.operation {
        ReplicationOperation::UpsertArtifact {
            kind,
            namespace_id,
            key,
            content_type,
            artifact_id,
            version_ms,
        } => {
            let manifest = match state.store.manifest(artifact_id)? {
                Some(manifest) => manifest,
                None => return Ok(()),
            };

            let file = state
                .store
                .open_artifact_reader(&manifest)
                .await
                .map_err(|error| {
                    format!("failed to open local artifact for replication: {error}")
                })?;

            let url = format!(
                "{}/_internal/replicate/artifact?kind={}&namespace_id={}&key={}&content_type={}&version_ms={}",
                message.target,
                kind.as_str(),
                url_encode(namespace_id),
                url_encode(key),
                url_encode(content_type),
                version_ms,
            );
            let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
            let request_span = tracing::info_span!(
                "replication.request",
                otel.name = "PUT /_internal/replicate/artifact",
                otel.kind = "client",
                kura.operation = "upsert_artifact",
                http.request.method = "PUT",
                url.full = %url,
                peer.service = %replication_target_label(&message.target),
                http.response.status_code = field::Empty,
                otel.status_code = field::Empty,
            );
            let response_span = request_span.clone();

            async {
                let mut headers = reqwest::header::HeaderMap::new();
                inject_current_trace_context(&mut headers);
                headers.insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/octet-stream"),
                );

                let response = state
                    .client
                    .put(&url)
                    .headers(headers)
                    .body(body)
                    .send()
                    .await
                    .map_err(|error| format!("artifact replication request failed: {error}"))?;
                response_span.record("http.response.status_code", response.status().as_u16());
                if response.status().is_server_error() {
                    response_span.record("otel.status_code", "ERROR");
                }
                response
                    .error_for_status()
                    .map(|_| ())
                    .map_err(|error| format!("artifact replication response failed: {error}"))
            }
            .instrument(request_span)
            .await
        }
        ReplicationOperation::DeleteNamespace {
            namespace_id,
            version_ms,
        } => {
            let url = format!(
                "{}/_internal/replicate/namespace?namespace_id={}&version_ms={}",
                message.target,
                url_encode(namespace_id),
                version_ms,
            );
            let request_span = tracing::info_span!(
                "replication.request",
                otel.name = "DELETE /_internal/replicate/namespace",
                otel.kind = "client",
                kura.operation = "delete_namespace",
                http.request.method = "DELETE",
                url.full = %url,
                peer.service = %replication_target_label(&message.target),
                http.response.status_code = field::Empty,
                otel.status_code = field::Empty,
            );
            let response_span = request_span.clone();

            async {
                let mut headers = reqwest::header::HeaderMap::new();
                inject_current_trace_context(&mut headers);
                let response = state
                    .client
                    .delete(&url)
                    .headers(headers)
                    .send()
                    .await
                    .map_err(|error| format!("namespace replication request failed: {error}"))?;
                response_span.record("http.response.status_code", response.status().as_u16());
                if response.status().is_server_error() {
                    response_span.record("otel.status_code", "ERROR");
                }
                response
                    .error_for_status()
                    .map(|_| ())
                    .map_err(|error| format!("namespace replication response failed: {error}"))
            }
            .instrument(request_span)
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use tokio::net::TcpListener;

    use super::*;
    use crate::{artifact::kind::ArtifactKind, http::router, test_support::test_context};

    async fn spawn_server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind test listener");
        let address = listener
            .local_addr()
            .expect("failed to read listener address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });
        (format!("http://{address}"), handle)
    }

    #[tokio::test]
    async fn enqueue_replication_skips_current_node() {
        let ctx = test_context(|config| {
            config.node_url = "http://127.0.0.1:4100".into();
            config.peers = vec![
                "http://127.0.0.1:4100".into(),
                "http://127.0.0.1:4101".into(),
            ];
        })
        .await;
        let manifest = ctx
            .state
            .store
            .persist_artifact_from_bytes(
                ArtifactKind::Xcode,
                "namespace",
                "artifact",
                "application/octet-stream",
                b"hello",
            )
            .await
            .expect("artifact should persist");

        enqueue_replication_for_artifact(&ctx.state, &manifest);

        let queued = ctx
            .state
            .store
            .outbox_messages()
            .expect("outbox should load");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].1.target, "http://127.0.0.1:4101");
    }

    #[tokio::test]
    async fn process_outbox_replicates_artifacts_and_namespace_deletes() {
        let remote = test_context(|_| {}).await;
        let (remote_url, _server) = spawn_server(router(remote.state.clone())).await;

        let local = test_context(|_| {}).await;
        local
            .state
            .store
            .persist_artifact_from_bytes(
                ArtifactKind::Gradle,
                "ios",
                "artifact",
                "application/octet-stream",
                b"payload",
            )
            .await
            .expect("artifact should persist");

        local
            .state
            .store
            .enqueue(OutboxMessage {
                target: remote_url.clone(),
                operation: ReplicationOperation::UpsertArtifact {
                    kind: ArtifactKind::Gradle,
                    namespace_id: "ios".into(),
                    key: "artifact".into(),
                    content_type: "application/octet-stream".into(),
                    artifact_id: local
                        .state
                        .store
                        .fetch_artifact(ArtifactKind::Gradle, "ios", "artifact")
                        .await
                        .expect("artifact fetch should succeed")
                        .expect("artifact should exist")
                        .artifact_id,
                    version_ms: local
                        .state
                        .store
                        .fetch_artifact(ArtifactKind::Gradle, "ios", "artifact")
                        .await
                        .expect("artifact fetch should succeed")
                        .expect("artifact should exist")
                        .version_ms,
                },
            })
            .expect("upsert should enqueue");

        local
            .state
            .store
            .enqueue(OutboxMessage {
                target: remote_url,
                operation: ReplicationOperation::DeleteNamespace {
                    namespace_id: "android".into(),
                    version_ms: 123,
                },
            })
            .expect("delete should enqueue");

        process_outbox(&local.state)
            .await
            .expect("outbox processing should succeed");

        let replicated = remote
            .state
            .store
            .fetch_artifact(ArtifactKind::Gradle, "ios", "artifact")
            .await
            .expect("artifact fetch should succeed")
            .expect("replicated artifact should exist");
        let mut reader = remote
            .state
            .store
            .open_artifact_reader(&replicated)
            .await
            .expect("replicated artifact reader should open");
        let mut bytes = Vec::new();
        use tokio::io::AsyncReadExt;
        reader
            .read_to_end(&mut bytes)
            .await
            .expect("replicated bytes should read");
        assert_eq!(bytes, b"payload");

        let queued = local
            .state
            .store
            .outbox_messages()
            .expect("outbox should load");
        assert!(
            queued.is_empty(),
            "successful replication should clear outbox"
        );
    }
}
