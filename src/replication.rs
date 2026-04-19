use std::{collections::BTreeSet, time::Duration};

use reqwest::header::{CONTENT_TYPE, HeaderValue};
use tokio::{fs, time::sleep};
use tokio_util::io::ReaderStream;
use tracing::{Instrument, field, warn};

use crate::{
    constants::REPLICATION_RETRY_SECS,
    domain::{ArtifactManifest, OutboxMessage, ReplicationOperation},
    state::SharedState,
    telemetry::inject_current_trace_context,
    utils::{replication_target_label, url_encode},
};

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
                project_handle: manifest.project_handle.clone(),
                key: manifest.key.clone(),
                content_type: manifest.content_type.clone(),
                artifact_id: manifest.artifact_id.clone(),
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
            if let Err(error) = process_outbox(&state).await {
                warn!("outbox processing failed: {error}");
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
            project_handle,
            key,
            content_type,
            artifact_id,
        } => {
            let manifest = match state.store.manifest(artifact_id)? {
                Some(manifest) => manifest,
                None => return Ok(()),
            };

            let file = fs::File::open(&manifest.blob_path).await.map_err(|error| {
                format!(
                    "failed to open local blob for replication {}: {error}",
                    manifest.blob_path
                )
            })?;

            let url = format!(
                "{}/_internal/replicate/artifact?kind={}&project_handle={}&key={}&content_type={}",
                message.target,
                kind.as_str(),
                url_encode(project_handle),
                url_encode(key),
                url_encode(content_type),
            );
            let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
            let request_span = tracing::info_span!(
                "replication.request",
                otel.name = "PUT /_internal/replicate/artifact",
                otel.kind = "client",
                cache.operation = "upsert_artifact",
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
        ReplicationOperation::DeleteProject { project_handle } => {
            let url = format!(
                "{}/_internal/replicate/project?project_handle={}",
                message.target,
                url_encode(project_handle),
            );
            let request_span = tracing::info_span!(
                "replication.request",
                otel.name = "DELETE /_internal/replicate/project",
                otel.kind = "client",
                cache.operation = "delete_project",
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
                    .map_err(|error| format!("project replication request failed: {error}"))?;
                response_span.record("http.response.status_code", response.status().as_u16());
                if response.status().is_server_error() {
                    response_span.record("otel.status_code", "ERROR");
                }
                response
                    .error_for_status()
                    .map(|_| ())
                    .map_err(|error| format!("project replication response failed: {error}"))
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
    use crate::{domain::ArtifactKind, http::router, test_support::test_context};

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
        let context = test_context(|config| {
            config.node_url = "http://local-node".into();
            config.peers = vec!["http://local-node".into(), "http://peer-node".into()];
        })
        .await;

        let manifest = context
            .state
            .store
            .persist_artifact_from_bytes(
                ArtifactKind::Xcode,
                "ios",
                "artifact-1",
                "application/octet-stream",
                b"xcode",
            )
            .expect("failed to persist seed artifact");

        enqueue_replication_for_artifact(&context.state, &manifest);

        let messages = context
            .state
            .store
            .outbox_messages()
            .expect("failed to read outbox messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1.target, "http://peer-node");
    }

    #[tokio::test]
    async fn process_outbox_replicates_artifacts_and_project_deletes() {
        let remote = test_context(|config| {
            config.region = "eu-west".into();
        })
        .await;
        let (remote_url, server_handle) = spawn_server(router(remote.state.clone())).await;

        let source = test_context(|config| {
            config.node_url = "http://local-node".into();
            config.peers = vec![remote_url.clone()];
        })
        .await;

        let manifest = source
            .state
            .store
            .persist_artifact_from_bytes(
                ArtifactKind::Keyvalue,
                "ios",
                "cas-1",
                "application/json",
                br#"{"cas_id":"cas-1","entries":[{"value":"hello"}]}"#,
            )
            .expect("failed to persist source artifact");
        enqueue_replication_for_artifact(&source.state, &manifest);

        process_outbox(&source.state)
            .await
            .expect("failed to process outbox");

        let replicated = remote
            .state
            .store
            .fetch_artifact(ArtifactKind::Keyvalue, "ios", "cas-1")
            .expect("failed to fetch replicated artifact")
            .expect("artifact should replicate");
        assert_eq!(
            std::fs::read_to_string(&replicated.blob_path).expect("failed to read replicated blob"),
            r#"{"cas_id":"cas-1","entries":[{"value":"hello"}]}"#
        );

        source
            .state
            .store
            .enqueue(OutboxMessage {
                target: remote_url,
                operation: ReplicationOperation::DeleteProject {
                    project_handle: "ios".into(),
                },
            })
            .expect("failed to enqueue delete message");

        process_outbox(&source.state)
            .await
            .expect("failed to process delete outbox");

        assert!(
            remote
                .state
                .store
                .fetch_artifact(ArtifactKind::Keyvalue, "ios", "cas-1")
                .expect("failed to fetch remote artifact")
                .is_none()
        );

        server_handle.abort();
    }
}
