use std::collections::HashMap;

use axum::{
    Json, Router,
    body::Body,
    extract::{MatchedPath, Path as AxumPath, Query, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, head, post, put},
};
use serde::Deserialize;
use tokio::fs;
use tokio_util::io::ReaderStream;
use tracing::{Instrument, field};

use crate::{
    artifact::{kind::ArtifactKind, manifest::ArtifactManifest},
    constants::{MAX_GRADLE_BYTES, MAX_MODULE_PART_BYTES, MAX_XCODE_BYTES},
    multipart::error::MultipartError,
    replication::enqueue_replication_for_artifact,
    replication::{operation::ReplicationOperation, outbox_message::OutboxMessage},
    state::SharedState,
    telemetry::attach_parent_context,
    utils::{BodyReadError, module_key, read_request_to_temp},
};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/up", get(up))
        .route("/metrics", get(metrics_handler))
        .route("/api/cache/keyvalue/{cas_id}", get(get_keyvalue))
        .route("/api/cache/keyvalue", put(put_keyvalue))
        .route("/api/cache/cas/{id}", get(get_xcode).post(put_xcode))
        .route("/api/cache/module/{id}", head(head_module).get(get_module))
        .route("/api/cache/module/start", post(start_module_upload))
        .route("/api/cache/module/part", post(upload_module_part))
        .route("/api/cache/module/complete", post(complete_module_upload))
        .route("/api/cache/clean", delete(clean_namespace))
        .route(
            "/api/cache/gradle/{cache_key}",
            get(get_gradle).put(put_gradle),
        )
        .route("/_internal/status", get(internal_status))
        .route(
            "/_internal/replicate/artifact",
            put(internal_replicate_artifact),
        )
        .route(
            "/_internal/replicate/namespace",
            delete(internal_delete_namespace),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            track_http_metrics,
        ))
        .with_state(state)
}

#[derive(Debug, PartialEq, Eq)]
struct NamespaceQuery {
    tenant_id: String,
    namespace_id: String,
}

impl NamespaceQuery {
    fn from_params(params: &HashMap<String, String>) -> Result<Self, String> {
        Ok(Self {
            tenant_id: required_param(params, "tenant_id")?,
            namespace_id: required_param(params, "namespace_id")?,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ModuleQuery {
    namespace: NamespaceQuery,
    cache_category: String,
    hash: String,
    name: String,
}

impl ModuleQuery {
    fn from_params(params: &HashMap<String, String>) -> Result<Self, String> {
        Ok(Self {
            namespace: NamespaceQuery::from_params(params)?,
            cache_category: params
                .get("cache_category")
                .cloned()
                .unwrap_or_else(|| "builds".into()),
            hash: required_param(params, "hash")?,
            name: required_param(params, "name")?,
        })
    }

    fn artifact_key(&self) -> String {
        module_key(&self.cache_category, &self.hash, &self.name)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct UploadPartQuery {
    upload_id: String,
    part_number: u32,
}

impl UploadPartQuery {
    fn from_params(params: &HashMap<String, String>) -> Result<Self, String> {
        let upload_id = required_param(params, "upload_id")?;
        let part_number = params
            .get("part_number")
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(|| "Invalid part_number".to_string())?;

        Ok(Self {
            upload_id,
            part_number,
        })
    }
}

#[derive(Debug, Deserialize)]
struct CompleteMultipartRequest {
    parts: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct KeyValuePutRequest {
    cas_id: String,
    entries: Vec<KeyValueEntry>,
}

#[derive(Debug, Deserialize)]
struct KeyValueEntry {
    value: String,
}

#[derive(Debug, PartialEq, Eq)]
struct UploadIdQuery {
    upload_id: String,
}

impl UploadIdQuery {
    fn from_params(params: &HashMap<String, String>) -> Result<Self, String> {
        Ok(Self {
            upload_id: required_param(params, "upload_id")?,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ReplicateArtifactQuery {
    kind: String,
    namespace_id: String,
    key: String,
    content_type: String,
}

impl ReplicateArtifactQuery {
    fn from_params(params: &HashMap<String, String>) -> Result<Self, String> {
        Ok(Self {
            kind: required_param(params, "kind")?,
            namespace_id: required_param(params, "namespace_id")?,
            key: required_param(params, "key")?,
            content_type: required_param(params, "content_type")?,
        })
    }
}

fn required_param(params: &HashMap<String, String>, key: &str) -> Result<String, String> {
    params
        .get(key)
        .cloned()
        .ok_or_else(|| format!("Missing {key}"))
}

async fn track_http_metrics(
    State(state): State<SharedState>,
    req: Request,
    next: Next,
) -> Response {
    let start = std::time::Instant::now();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned());
    let method = req.method().to_string();
    let uri_path = req.uri().path().to_owned();

    let request_span = tracing::info_span!(
        "http.request",
        otel.name = %format!("{method} {route}"),
        otel.kind = "server",
        http.request.method = %method,
        http.route = %route,
        url.path = %uri_path,
        http.response.status_code = field::Empty,
        otel.status_code = field::Empty,
    );
    attach_parent_context(&request_span, req.headers());

    let response = next.run(req).instrument(request_span.clone()).await;
    request_span.record("http.response.status_code", response.status().as_u16());
    if response.status().is_server_error() {
        request_span.record("otel.status_code", "ERROR");
    }

    state
        .metrics
        .record_http(route, method, response.status(), start.elapsed());

    response
}

async fn up(State(state): State<SharedState>) -> impl IntoResponse {
    let members = state.members.read().await.clone();
    let mut all_members = members;
    all_members.insert(state.config.region.clone());

    Json(serde_json::json!({
        "status": "ok",
        "tenant_id": state.config.tenant_id.clone(),
        "region": state.config.region.clone(),
        "node": state.config.region.clone(),
        "connected_nodes": all_members.iter().cloned().filter(|region| region != &state.config.region).collect::<Vec<_>>(),
        "ring_members": all_members.len(),
        "members": all_members.into_iter().collect::<Vec<_>>(),
    }))
}

async fn metrics_handler(State(state): State<SharedState>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4"),
        )],
        state.metrics.render(),
    )
}

async fn get_keyvalue(
    AxumPath(cas_id): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let namespace = match NamespaceQuery::from_params(&params) {
        Ok(namespace) => namespace,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    get_artifact(
        state,
        ArtifactKind::Keyvalue,
        &namespace.namespace_id,
        &cas_id,
    )
    .await
}

async fn put_keyvalue(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
    Json(body): Json<KeyValuePutRequest>,
) -> Response {
    let namespace = match NamespaceQuery::from_params(&params) {
        Ok(namespace) => namespace,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let cas_id = body.cas_id.clone();
    let payload = serde_json::json!({
        "cas_id": body.cas_id,
        "entries": body.entries.into_iter().map(|entry| serde_json::json!({ "value": entry.value })).collect::<Vec<_>>()
    });
    let payload_bytes = payload.to_string();

    match state.store.persist_artifact_from_bytes(
        ArtifactKind::Keyvalue,
        &namespace.namespace_id,
        &cas_id,
        "application/json",
        payload_bytes.as_bytes(),
    ) {
        Ok(manifest) => {
            enqueue_replication_for_artifact(&state, &manifest);
            state
                .metrics
                .record_artifact_write(ArtifactKind::Keyvalue, "ok", manifest.size);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => {
            state
                .metrics
                .record_artifact_write(ArtifactKind::Keyvalue, "error", 0);
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Failed to persist key-value entry: {error}"),
            )
        }
    }
}

async fn get_xcode(
    AxumPath(id): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let namespace = match NamespaceQuery::from_params(&params) {
        Ok(namespace) => namespace,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    get_artifact(state, ArtifactKind::Xcode, &namespace.namespace_id, &id).await
}

async fn put_xcode(
    AxumPath(id): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
    request: Request,
) -> Response {
    let namespace = match NamespaceQuery::from_params(&params) {
        Ok(namespace) => namespace,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    put_blob_artifact(
        state,
        ArtifactKind::Xcode,
        &namespace.namespace_id,
        &id,
        request,
        MAX_XCODE_BYTES,
        StatusCode::NO_CONTENT,
    )
    .await
}

async fn get_gradle(
    AxumPath(cache_key): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let namespace = match NamespaceQuery::from_params(&params) {
        Ok(namespace) => namespace,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    get_artifact(
        state,
        ArtifactKind::Gradle,
        &namespace.namespace_id,
        &cache_key,
    )
    .await
}

async fn put_gradle(
    AxumPath(cache_key): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
    request: Request,
) -> Response {
    let namespace = match NamespaceQuery::from_params(&params) {
        Ok(namespace) => namespace,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    put_blob_artifact(
        state,
        ArtifactKind::Gradle,
        &namespace.namespace_id,
        &cache_key,
        request,
        MAX_GRADLE_BYTES,
        StatusCode::CREATED,
    )
    .await
}

async fn head_module(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let query = match ModuleQuery::from_params(&params) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state.store.artifact_exists(
        ArtifactKind::Module,
        &query.namespace.namespace_id,
        &query.artifact_key(),
    ) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("Failed to inspect artifact: {error}"),
        ),
    }
}

async fn get_module(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let query = match ModuleQuery::from_params(&params) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    get_artifact(
        state,
        ArtifactKind::Module,
        &query.namespace.namespace_id,
        &query.artifact_key(),
    )
    .await
}

async fn start_module_upload(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let query = match ModuleQuery::from_params(&params) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state.store.artifact_exists(
        ArtifactKind::Module,
        &query.namespace.namespace_id,
        &query.artifact_key(),
    ) {
        Ok(true) => {
            Json(serde_json::json!({ "upload_id": serde_json::Value::Null })).into_response()
        }
        Ok(false) => match state.store.start_multipart_upload(
            &query.namespace.tenant_id,
            &query.namespace.namespace_id,
            &query.cache_category,
            &query.hash,
            &query.name,
        ) {
            Ok(upload_id) => Json(serde_json::json!({ "upload_id": upload_id })).into_response(),
            Err(error) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to start upload: {error}"),
            ),
        },
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("Failed to inspect artifact: {error}"),
        ),
    }
}

async fn upload_module_part(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
    request: Request,
) -> Response {
    let query = match UploadPartQuery::from_params(&params) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let temp = match read_request_to_temp(
        request,
        &state.config.tmp_dir.join("parts"),
        MAX_MODULE_PART_BYTES,
    )
    .await
    {
        Ok(temp) => temp,
        Err(BodyReadError::TooLarge) => {
            return error_response(StatusCode::PAYLOAD_TOO_LARGE, "Part exceeds 10MB limit");
        }
        Err(BodyReadError::Io(error)) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to persist multipart upload part: {error}"),
            );
        }
    };

    match state
        .store
        .add_multipart_part(&query.upload_id, query.part_number, &temp.path, temp.size)
    {
        Ok(()) => {
            state.metrics.record_multipart_part("ok");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(MultipartError::NotFound) => {
            let _ = std::fs::remove_file(&temp.path);
            state.metrics.record_multipart_part("not_found");
            error_response(StatusCode::NOT_FOUND, "Upload not found")
        }
        Err(MultipartError::TotalSizeExceeded) => {
            let _ = std::fs::remove_file(&temp.path);
            state.metrics.record_multipart_part("too_large");
            error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Total upload size exceeds 2GB limit",
            )
        }
        Err(MultipartError::Other(error)) => {
            let _ = std::fs::remove_file(&temp.path);
            state.metrics.record_multipart_part("error");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to store multipart upload part: {error}"),
            )
        }
        Err(MultipartError::PartsMismatch) => {
            let _ = std::fs::remove_file(&temp.path);
            state.metrics.record_multipart_part("parts_mismatch");
            error_response(StatusCode::BAD_REQUEST, "Parts mismatch")
        }
    }
}

async fn complete_module_upload(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
    Json(body): Json<CompleteMultipartRequest>,
) -> Response {
    let query = match UploadIdQuery::from_params(&params) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state
        .store
        .complete_multipart_upload(&query.upload_id, &body.parts)
    {
        Ok(manifest) => {
            enqueue_replication_for_artifact(&state, &manifest);
            state
                .metrics
                .record_artifact_write(ArtifactKind::Module, "ok", manifest.size);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(MultipartError::NotFound) => error_response(StatusCode::NOT_FOUND, "Upload not found"),
        Err(MultipartError::PartsMismatch) => {
            error_response(StatusCode::BAD_REQUEST, "Parts mismatch or missing parts")
        }
        Err(MultipartError::TotalSizeExceeded) => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Total upload size exceeds 2GB limit",
        ),
        Err(MultipartError::Other(error)) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to complete multipart upload: {error}"),
        ),
    }
}

async fn clean_namespace(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let namespace = match NamespaceQuery::from_params(&params) {
        Ok(namespace) => namespace,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state.store.delete_namespace(&namespace.namespace_id) {
        Ok(()) => {
            for peer in state
                .config
                .peers
                .iter()
                .filter(|peer| *peer != &state.config.node_url)
            {
                if let Err(error) = state.store.enqueue(OutboxMessage {
                    target: peer.clone(),
                    operation: ReplicationOperation::DeleteNamespace {
                        namespace_id: namespace.namespace_id.clone(),
                    },
                }) {
                    tracing::warn!("failed to enqueue namespace delete for {peer}: {error}");
                }
            }
            state.notify.notify_one();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to clean cache: {error}"),
        ),
    }
}

async fn internal_status(State(state): State<SharedState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "region": state.config.region.clone(),
        "tenant_id": state.config.tenant_id.clone(),
    }))
}

async fn internal_replicate_artifact(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
    request: Request,
) -> Response {
    let query = match ReplicateArtifactQuery::from_params(&params) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let kind = match ArtifactKind::from_str(&query.kind) {
        Some(kind) => kind,
        None => return error_response(StatusCode::BAD_REQUEST, "Invalid artifact kind"),
    };

    let temp = match read_request_to_temp(request, &state.config.tmp_dir.join("uploads"), u64::MAX)
        .await
    {
        Ok(temp) => temp,
        Err(BodyReadError::TooLarge) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body exceeded allowed size",
            );
        }
        Err(BodyReadError::Io(error)) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read replication body: {error}"),
            );
        }
    };

    match state.store.persist_artifact_from_path(
        kind,
        &query.namespace_id,
        &query.key,
        &query.content_type,
        &temp.path,
    ) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to persist replicated artifact: {error}"),
        ),
    }
}

async fn internal_delete_namespace(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let namespace_id = match required_param(&params, "namespace_id") {
        Ok(namespace_id) => namespace_id,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state.store.delete_namespace(&namespace_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to delete replicated namespace: {error}"),
        ),
    }
}

async fn get_artifact(
    state: SharedState,
    kind: ArtifactKind,
    namespace_id: &str,
    key: &str,
) -> Response {
    match state.store.fetch_artifact(kind, namespace_id, key) {
        Ok(Some(manifest)) => {
            state
                .metrics
                .record_artifact_read(kind, "ok", manifest.size);
            serve_file(StatusCode::OK, &manifest).await
        }
        Ok(None) => {
            state.metrics.record_artifact_read(kind, "not_found", 0);
            StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => {
            state.metrics.record_artifact_read(kind, "error", 0);
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Failed to fetch artifact: {error}"),
            )
        }
    }
}

async fn put_blob_artifact(
    state: SharedState,
    kind: ArtifactKind,
    namespace_id: &str,
    key: &str,
    request: Request,
    max_bytes: u64,
    success_status: StatusCode,
) -> Response {
    match state.store.artifact_exists(kind, namespace_id, key) {
        Ok(true) => return success_status.into_response(),
        Ok(false) => {}
        Err(error) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Failed to inspect artifact: {error}"),
            );
        }
    }

    let temp = match read_request_to_temp(request, &state.config.tmp_dir.join("uploads"), max_bytes)
        .await
    {
        Ok(temp) => temp,
        Err(BodyReadError::TooLarge) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body exceeded allowed size",
            );
        }
        Err(BodyReadError::Io(error)) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to persist artifact: {error}"),
            );
        }
    };

    match state.store.persist_artifact_from_path(
        kind,
        namespace_id,
        key,
        "application/octet-stream",
        &temp.path,
    ) {
        Ok(manifest) => {
            enqueue_replication_for_artifact(&state, &manifest);
            state
                .metrics
                .record_artifact_write(kind, "ok", manifest.size);
            success_status.into_response()
        }
        Err(error) => {
            state.metrics.record_artifact_write(kind, "error", 0);
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Failed to persist artifact: {error}"),
            )
        }
    }
}

async fn serve_file(status: StatusCode, manifest: &ArtifactManifest) -> Response {
    match fs::File::open(&manifest.blob_path).await {
        Ok(file) => {
            let stream = ReaderStream::new(file);
            let mut response = Response::new(Body::from_stream(stream));
            *response.status_mut() = status;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_str(&manifest.content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            );
            response
        }
        Err(error) => error_response(
            StatusCode::NOT_FOUND,
            format!("Artifact blob is missing from local disk: {error}"),
        ),
    }
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = Json(serde_json::json!({ "message": message.into() }));
    (status, body).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::test_support::{response_text, test_context};

    #[tokio::test]
    async fn up_includes_current_node_and_known_members() {
        let context = test_context(|config| {
            config.region = "us-east".into();
        })
        .await;
        context.state.members.write().await.insert("eu-west".into());

        let response = router(context.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/up")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_str(&response_text(response).await)
            .expect("failed to decode up response");
        assert_eq!(body["ring_members"], 2);
        assert_eq!(body["region"], "us-east");
        assert!(body["members"].to_string().contains("eu-west"));
    }

    #[tokio::test]
    async fn keyvalue_round_trip_works_through_router() {
        let context = test_context(|_| {}).await;
        let app = router(context.state.clone());

        let put_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/cache/keyvalue?tenant_id=acme&namespace_id=ios")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"cas_id":"cas-1","entries":[{"value":"hello"},{"value":"world"}]}"#,
                    ))
                    .expect("failed to build put request"),
            )
            .await
            .expect("put request failed");
        assert_eq!(put_response.status(), StatusCode::NO_CONTENT);

        let get_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/cache/keyvalue/cas-1?tenant_id=acme&namespace_id=ios")
                    .body(Body::empty())
                    .expect("failed to build get request"),
            )
            .await
            .expect("get request failed");
        assert_eq!(get_response.status(), StatusCode::OK);

        let body: Value = serde_json::from_str(&response_text(get_response).await)
            .expect("failed to decode keyvalue response");
        assert_eq!(body["cas_id"], "cas-1");
        assert_eq!(body["entries"][0]["value"], "hello");
        assert_eq!(body["entries"][1]["value"], "world");
    }

    #[tokio::test]
    async fn multipart_module_round_trip_works_through_router() {
        let context = test_context(|_| {}).await;
        let app = router(context.state.clone());

        let start = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/cache/module/start?tenant_id=acme&namespace_id=ios&hash=hash-1&name=Module.framework&cache_category=builds")
                    .body(Body::empty())
                    .expect("failed to build start request"),
            )
            .await
            .expect("start request failed");
        let payload: Value = serde_json::from_str(&response_text(start).await)
            .expect("failed to decode start payload");
        let upload_id = payload["upload_id"]
            .as_str()
            .expect("upload id should be present");

        let upload_part = |part_number, body| {
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/cache/module/part?upload_id={upload_id}&part_number={part_number}"
                ))
                .body(Body::from(body))
                .expect("failed to build part request")
        };

        let response = app
            .clone()
            .oneshot(upload_part(1, "part-one-"))
            .await
            .expect("part 1 request failed");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(upload_part(2, "part-two"))
            .await
            .expect("part 2 request failed");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/cache/module/complete?upload_id={upload_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"parts":[1,2]}"#))
                    .expect("failed to build complete request"),
            )
            .await
            .expect("complete request failed");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let head = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/api/cache/module/module-1?tenant_id=acme&namespace_id=ios&hash=hash-1&name=Module.framework&cache_category=builds")
                    .body(Body::empty())
                    .expect("failed to build head request"),
            )
            .await
            .expect("head request failed");
        assert_eq!(head.status(), StatusCode::NO_CONTENT);

        let get = app
            .oneshot(
                Request::builder()
                    .uri("/api/cache/module/module-1?tenant_id=acme&namespace_id=ios&hash=hash-1&name=Module.framework&cache_category=builds")
                    .body(Body::empty())
                    .expect("failed to build get request"),
            )
            .await
            .expect("get request failed");
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(response_text(get).await, "part-one-part-two");
    }

    #[tokio::test]
    async fn missing_required_query_returns_json_error() {
        let context = test_context(|_| {}).await;

        let response = router(context.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/cache/keyvalue/cas-1?namespace_id=ios")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::from_str::<Value>(&response_text(response).await)
                .expect("failed to decode error response")["message"],
            "Missing tenant_id"
        );
    }

    #[tokio::test]
    async fn clean_namespace_removes_existing_artifacts() {
        let context = test_context(|_| {}).await;
        context
            .state
            .store
            .persist_artifact_from_bytes(
                ArtifactKind::Xcode,
                "ios",
                "artifact-1",
                "application/octet-stream",
                b"xcode-binary",
            )
            .expect("failed to seed store");

        let app = router(context.state.clone());

        let delete = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/cache/clean?tenant_id=acme&namespace_id=ios")
                    .body(Body::empty())
                    .expect("failed to build delete request"),
            )
            .await
            .expect("delete request failed");
        assert_eq!(delete.status(), StatusCode::NO_CONTENT);

        let get = app
            .oneshot(
                Request::builder()
                    .uri("/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios")
                    .body(Body::empty())
                    .expect("failed to build get request"),
            )
            .await
            .expect("get request failed");
        assert_eq!(get.status(), StatusCode::NOT_FOUND);
    }
}
