pub mod operation;
pub mod outbox_message;

use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    path::Path,
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderValue};
use serde::Deserialize;
use tokio::{io::AsyncWriteExt, time::sleep};
use tokio_util::io::ReaderStream;
use tracing::{Instrument, field, warn};

use crate::{
    artifact::manifest::ArtifactManifest,
    config::Config,
    constants::REPLICATION_RETRY_SECS,
    state::SharedState,
    store::{ManifestPage, NamespaceTombstonePage},
    telemetry::inject_current_trace_context,
    utils::{replication_target_label, temp_file_path, url_encode},
};

use self::{operation::ReplicationOperation, outbox_message::OutboxMessage};

const BOOTSTRAP_PAGE_LIMIT: usize = 256;

#[derive(Debug, Deserialize)]
struct PeerStatusPayload {
    region: String,
    tenant_id: String,
    node_url: String,
}

#[cfg(test)]
pub async fn enqueue_replication_for_artifact(state: &SharedState, manifest: &ArtifactManifest) {
    for peer in replication_targets(state).await {
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
            let mut peer_nodes = BTreeMap::new();
            for peer in discovery_targets(&state.config).await {
                match state
                    .client
                    .get(format!("{peer}/_internal/status"))
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => match response
                        .json::<PeerStatusPayload>()
                        .await
                    {
                        Ok(payload) => {
                            if payload.tenant_id != state.config.tenant_id
                                || payload.node_url == state.config.node_url
                            {
                                continue;
                            }
                            members.insert(payload.region.clone());
                            peer_nodes.insert(payload.node_url, payload.region);
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
            let (discovered_peers, lost_peers) = {
                let mut known_peers = state.peer_nodes.write().await;
                let lost_peers = known_peers
                    .keys()
                    .filter(|peer| !peer_nodes.contains_key(*peer))
                    .cloned()
                    .collect::<Vec<_>>();
                let discovered_peers = peer_nodes
                    .keys()
                    .filter(|peer| !known_peers.contains_key(*peer))
                    .cloned()
                    .collect::<Vec<_>>();
                *known_peers = peer_nodes;
                (discovered_peers, lost_peers)
            };
            if !lost_peers.is_empty() {
                let mut bootstrapped = state.bootstrapped_peers.lock().await;
                for peer in lost_peers {
                    bootstrapped.remove(&peer);
                }
            }
            state
                .metrics
                .update_discovered_peer_nodes(state.peer_nodes.read().await.len());
            for peer in discovered_peers {
                maybe_spawn_bootstrap_task(state.clone(), peer).await;
            }
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
            if !pause_outbox && let Err(error) = process_outbox(&state).await {
                warn!("outbox processing failed: {error}");
            }

            tokio::select! {
                _ = state.notify.notified() => {},
                _ = sleep(Duration::from_secs(REPLICATION_RETRY_SECS)) => {},
            }
        }
    });
}

pub async fn replication_targets(state: &SharedState) -> Vec<String> {
    let mut targets = state.config.peers.iter().cloned().collect::<BTreeSet<_>>();
    targets.extend(state.peer_nodes.read().await.keys().cloned());
    targets.remove(&state.config.node_url);
    targets.into_iter().collect()
}

async fn maybe_spawn_bootstrap_task(state: SharedState, peer: String) {
    let mut bootstrapped = state.bootstrapped_peers.lock().await;
    if !bootstrapped.insert(peer.clone()) {
        return;
    }
    drop(bootstrapped);

    tokio::spawn(async move {
        let started_at = std::time::Instant::now();
        let result = bootstrap_from_peer(&state, &peer).await;
        match result {
            Ok(stats) => {
                state.metrics.record_bootstrap_run(
                    "ok",
                    started_at.elapsed(),
                    stats.tombstones_applied,
                    stats.artifacts_applied,
                );
            }
            Err(error) => {
                warn!("bootstrap from {peer} failed: {error}");
                state
                    .metrics
                    .record_bootstrap_run("error", started_at.elapsed(), 0, 0);
                state.bootstrapped_peers.lock().await.remove(&peer);
            }
        }
    });
}

async fn bootstrap_from_peer(state: &SharedState, peer: &str) -> Result<BootstrapStats, String> {
    let tombstones_applied = bootstrap_namespace_tombstones_from_peer(state, peer).await?;
    let artifacts_applied = bootstrap_manifests_from_peer(state, peer).await?;
    Ok(BootstrapStats {
        tombstones_applied,
        artifacts_applied,
    })
}

async fn bootstrap_namespace_tombstones_from_peer(
    state: &SharedState,
    peer: &str,
) -> Result<u64, String> {
    let mut after = None;
    let mut applied = 0_u64;

    loop {
        let page = fetch_bootstrap_tombstones_page(state, peer, after.as_deref()).await?;
        for tombstone in &page.tombstones {
            if state
                .store
                .apply_replicated_namespace_delete(&tombstone.namespace_id, tombstone.version_ms)
                .await?
            {
                applied += 1;
            }
        }

        match page.next_after {
            Some(next_after) => after = Some(next_after),
            None => return Ok(applied),
        }
    }
}

async fn bootstrap_manifests_from_peer(state: &SharedState, peer: &str) -> Result<u64, String> {
    let mut after = None;
    let mut applied = 0_u64;

    loop {
        let page = fetch_bootstrap_manifests_page(state, peer, after.as_deref()).await?;
        for manifest in &page.manifests {
            if !state.store.artifact_version_is_current(
                manifest.kind,
                &manifest.namespace_id,
                &manifest.key,
                manifest.version_ms,
            )? {
                continue;
            }

            if bootstrap_artifact_from_peer(state, peer, manifest).await? {
                applied += 1;
            }
        }

        match page.next_after {
            Some(next_after) => after = Some(next_after),
            None => return Ok(applied),
        }
    }
}

async fn bootstrap_artifact_from_peer(
    state: &SharedState,
    peer: &str,
    manifest: &ArtifactManifest,
) -> Result<bool, String> {
    let url = format!(
        "{peer}/_internal/bootstrap/artifacts/{}",
        url_encode(&manifest.artifact_id)
    );
    let response = state
        .client
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("bootstrap artifact request failed: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("bootstrap artifact response failed: {error}"))?;

    if manifest.kind == crate::artifact::kind::ArtifactKind::Keyvalue {
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("failed to read bootstrap keyvalue body: {error}"))?;
        return state
            .store
            .apply_replicated_artifact_from_bytes(
                manifest.kind,
                &manifest.namespace_id,
                &manifest.key,
                &manifest.content_type,
                bytes.as_ref(),
                manifest.version_ms,
            )
            .await;
    }

    let temp_path = temp_file_path(&state.config.tmp_dir.join("bootstrap"), "bootstrap");
    stream_response_to_temp(state, response, &temp_path).await?;
    state
        .store
        .apply_replicated_artifact_from_path(
            manifest.kind,
            &manifest.namespace_id,
            &manifest.key,
            &manifest.content_type,
            &temp_path,
            manifest.version_ms,
        )
        .await
}

async fn stream_response_to_temp(
    state: &SharedState,
    response: reqwest::Response,
    path: &Path,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "bootstrap temp path is missing a parent directory".to_string())?;
    state.io.create_dir_all(parent).await?;
    let mut destination = state.io.create_file(path).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("failed to stream bootstrap body: {error}"))?;
        destination
            .write_all(&chunk)
            .await
            .map_err(|error| format!("failed to persist bootstrap body: {error}"))?;
    }
    destination
        .flush()
        .await
        .map_err(|error| format!("failed to flush bootstrap body: {error}"))?;
    Ok(())
}

async fn fetch_bootstrap_manifests_page(
    state: &SharedState,
    peer: &str,
    after: Option<&str>,
) -> Result<ManifestPage, String> {
    let mut url = format!("{peer}/_internal/bootstrap/manifests?limit={BOOTSTRAP_PAGE_LIMIT}");
    if let Some(after) = after {
        url.push_str("&after=");
        url.push_str(&url_encode(after));
    }

    state
        .client
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("bootstrap manifest request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("bootstrap manifest response failed: {error}"))?
        .json::<ManifestPage>()
        .await
        .map_err(|error| format!("failed to decode bootstrap manifest page: {error}"))
}

async fn fetch_bootstrap_tombstones_page(
    state: &SharedState,
    peer: &str,
    after: Option<&str>,
) -> Result<NamespaceTombstonePage, String> {
    let mut url =
        format!("{peer}/_internal/bootstrap/namespace_tombstones?limit={BOOTSTRAP_PAGE_LIMIT}");
    if let Some(after) = after {
        url.push_str("&after=");
        url.push_str(&url_encode(after));
    }

    state
        .client
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("bootstrap tombstone request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("bootstrap tombstone response failed: {error}"))?
        .json::<NamespaceTombstonePage>()
        .await
        .map_err(|error| format!("failed to decode bootstrap tombstone page: {error}"))
}

async fn discovery_targets(config: &Config) -> Vec<String> {
    let mut targets = config.peers.iter().cloned().collect::<BTreeSet<_>>();
    let Some(dns_name) = &config.discovery_dns_name else {
        return targets.into_iter().collect();
    };

    let Ok(node_url) = reqwest::Url::parse(&config.node_url) else {
        return targets.into_iter().collect();
    };
    let Some(port) = node_url.port_or_known_default() else {
        return targets.into_iter().collect();
    };
    let scheme = node_url.scheme().to_owned();
    if scheme == "https" {
        targets.insert(format!("{scheme}://{dns_name}:{port}"));
        return targets.into_iter().collect();
    }

    match tokio::net::lookup_host((dns_name.as_str(), port)).await {
        Ok(addresses) => {
            for address in addresses {
                targets.insert(format!(
                    "{scheme}://{}:{port}",
                    format_ip_for_url(address.ip())
                ));
            }
        }
        Err(error) => warn!("dns discovery lookup failed for {dns_name}:{port}: {error}"),
    }

    targets.into_iter().collect()
}

fn format_ip_for_url(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

struct BootstrapStats {
    tombstones_applied: u64,
    artifacts_applied: u64,
}

pub async fn process_outbox(state: &SharedState) -> Result<(), String> {
    let mut after = None::<Vec<u8>>;
    while let Some((message_key, message)) = state.store.next_outbox_message(after.as_deref())? {
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
        after = Some(message_key);
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

        enqueue_replication_for_artifact(&ctx.state, &manifest).await;

        let queued = ctx
            .state
            .store
            .outbox_messages()
            .expect("outbox should load");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].1.target, "http://127.0.0.1:4101");
    }

    #[tokio::test]
    async fn discover_targets_keeps_dns_names_for_https_peers() {
        let ctx = test_context(|config| {
            config.node_url = "https://kura-us.kura.internal:7443".into();
            config.peers = vec!["https://seed.kura.internal:7443".into()];
            config.discovery_dns_name = Some("kura-ring.kura.internal".into());
        })
        .await;

        let targets: Vec<String> = discovery_targets(&ctx.state.config).await;

        assert!(targets.contains(&"https://seed.kura.internal:7443".to_string()));
        assert!(targets.contains(&"https://kura-ring.kura.internal:7443".to_string()));
        assert!(!targets.iter().any(|target: &String| {
            target.starts_with("https://127.") || target.starts_with("https://[::1]")
        }));
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
