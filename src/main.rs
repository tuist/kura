use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{MatchedPath, Path as AxumPath, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, head, post, put},
};
use futures_util::StreamExt;
use opentelemetry::{
    KeyValue, global,
    propagation::{Extractor, Injector},
    trace::TracerProvider as _,
};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    propagation::TraceContextPropagator,
    trace::{Sampler, SdkTracerProvider},
};
use prometheus_client::{
    encoding::{EncodeLabelSet, text::encode},
    metrics::{
        counter::Counter,
        family::Family,
        gauge::Gauge,
        histogram::{Histogram, exponential_buckets},
    },
    registry::Registry,
};
use reqwest::Client;
use rocksdb::{ColumnFamily, ColumnFamilyDescriptor, DB, IteratorMode, Options, WriteBatch};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{Notify, RwLock},
    time::sleep,
};
use tokio_util::io::ReaderStream;
use tracing::{Instrument, Span, field, info, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

const MAX_XCODE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_GRADLE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_MODULE_PART_BYTES: u64 = 10 * 1024 * 1024;
const MAX_MODULE_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const REPLICATION_RETRY_SECS: u64 = 2;

const CF_MANIFESTS: &str = "manifests";
const CF_PROJECT_ARTIFACTS: &str = "project_artifacts";
const CF_MULTIPART_UPLOADS: &str = "multipart_uploads";
const CF_OUTBOX: &str = "outbox";

#[tokio::main]
async fn main() {
    let config = Arc::new(Config::from_env());
    let tracer_provider = init_tracing(config.as_ref());

    if let Err(error) = config.ensure_directories().await {
        panic!("failed to create directories: {error}");
    }

    let store = Arc::new(Store::open(config.clone()).expect("failed to open RocksDB"));
    let metrics_recorder = Arc::new(Metrics::new(config.region.clone(), config.tenant.clone()));
    let members = Arc::new(RwLock::new(BTreeSet::new()));
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");
    let notify = Arc::new(Notify::new());

    let state = Arc::new(AppState {
        config,
        store,
        metrics: metrics_recorder,
        client,
        notify,
        members,
    });

    spawn_membership_task(state.clone());
    spawn_outbox_task(state.clone());

    let router = Router::new()
        .route("/up", get(up))
        .route("/metrics", get(metrics_handler))
        .route("/api/cache/keyvalue/{cas_id}", get(get_keyvalue))
        .route("/api/cache/keyvalue", put(put_keyvalue))
        .route("/api/cache/cas/{id}", get(get_xcode).post(put_xcode))
        .route("/api/cache/module/{id}", head(head_module).get(get_module))
        .route("/api/cache/module/start", post(start_module_upload))
        .route("/api/cache/module/part", post(upload_module_part))
        .route("/api/cache/module/complete", post(complete_module_upload))
        .route("/api/cache/clean", delete(clean_project))
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
            "/_internal/replicate/project",
            delete(internal_delete_project),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            track_http_metrics,
        ))
        .with_state(state.clone());

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, state.config.port));
    info!("cache Rust service listening on {address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind TCP listener");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");

    if let Some(provider) = tracer_provider {
        if let Err(error) = provider.shutdown() {
            eprintln!("failed to shutdown OTLP tracer provider: {error}");
        }
    }
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

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    store: Arc<Store>,
    metrics: Arc<Metrics>,
    client: Client,
    notify: Arc<Notify>,
    members: Arc<RwLock<BTreeSet<String>>>,
}

#[derive(Clone, Debug)]
struct Config {
    port: u16,
    tenant: String,
    region: String,
    tmp_dir: PathBuf,
    data_dir: PathBuf,
    node_url: String,
    peers: Vec<String>,
    otlp_traces_endpoint: Option<String>,
    otel_service_name: String,
    otel_deployment_environment: String,
}

impl Config {
    fn from_env() -> Self {
        let port = std::env::var("PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(4000);
        let tenant = std::env::var("TENANT_ID").unwrap_or_else(|_| "demo-tenant".into());
        let region = std::env::var("CACHE_REGION").unwrap_or_else(|_| "local".into());
        let tmp_dir = std::env::var("CACHE_TMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("tmp/cache"));
        let data_dir = std::env::var("CACHE_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("tmp/cache-data"));
        let node_url =
            std::env::var("CACHE_NODE_URL").unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));
        let peers = std::env::var("CACHE_PEERS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let otlp_traces_endpoint = std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
            .ok()
            .or_else(|| {
                std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                    .ok()
                    .map(|value| format!("{}/v1/traces", value.trim_end_matches('/')))
            });
        let otel_service_name = std::env::var("OTEL_SERVICE_NAME")
            .unwrap_or_else(|_| format!("cache-{}", region.replace('_', "-")));
        let otel_deployment_environment =
            std::env::var("OTEL_DEPLOYMENT_ENVIRONMENT").unwrap_or_else(|_| "local".into());

        Self {
            port,
            tenant,
            region,
            tmp_dir,
            data_dir,
            node_url,
            peers,
            otlp_traces_endpoint,
            otel_service_name,
            otel_deployment_environment,
        }
    }

    async fn ensure_directories(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(self.tmp_dir.join("uploads")).await?;
        fs::create_dir_all(self.tmp_dir.join("parts")).await?;
        fs::create_dir_all(self.data_dir.join("rocksdb")).await?;
        fs::create_dir_all(self.data_dir.join("blobs")).await?;
        fs::create_dir_all(self.data_dir.join("multipart")).await?;
        Ok(())
    }
}

fn init_tracing(config: &Config) -> Option<SdkTracerProvider> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "cache=info".into());
    let fmt_layer = tracing_subscriber::fmt::layer();

    match config.otlp_traces_endpoint.as_deref() {
        Some(endpoint) => match build_tracer_provider(config, endpoint) {
            Ok(tracer_provider) => {
                let tracer = tracer_provider.tracer("cache");
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(fmt_layer)
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .init();
                Some(tracer_provider)
            }
            Err(error) => {
                eprintln!("failed to initialize OTLP tracing, falling back to logs only: {error}");
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(fmt_layer)
                    .init();
                None
            }
        },
        None => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .init();
            None
        }
    }
}

fn build_tracer_provider(config: &Config, endpoint: &str) -> Result<SdkTracerProvider, String> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(Duration::from_secs(3))
        .build()
        .map_err(|error| format!("failed to build OTLP exporter: {error}"))?;

    let resource = Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", config.otel_service_name.clone()),
            KeyValue::new("service.namespace", "cache"),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new(
                "deployment.environment.name",
                config.otel_deployment_environment.clone(),
            ),
            KeyValue::new("cache.region", config.region.clone()),
            KeyValue::new("cache.tenant", config.tenant.clone()),
            KeyValue::new("service.instance.id", config.node_url.clone()),
        ])
        .build();

    Ok(SdkTracerProvider::builder()
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            1.0,
        ))))
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ArtifactManifest {
    artifact_id: String,
    kind: ArtifactKind,
    project_handle: String,
    key: String,
    content_type: String,
    blob_path: String,
    size: u64,
    created_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ArtifactKind {
    Keyvalue,
    Xcode,
    Gradle,
    Module,
}

impl ArtifactKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Keyvalue => "keyvalue",
            Self::Xcode => "xcode",
            Self::Gradle => "gradle",
            Self::Module => "module",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "keyvalue" => Some(Self::Keyvalue),
            "xcode" => Some(Self::Xcode),
            "gradle" => Some(Self::Gradle),
            "module" => Some(Self::Module),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MultipartUpload {
    upload_id: String,
    account_handle: String,
    project_handle: String,
    category: String,
    hash: String,
    name: String,
    parts: BTreeMap<u32, MultipartPart>,
    created_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MultipartPart {
    path: String,
    size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OutboxMessage {
    target: String,
    operation: ReplicationOperation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReplicationOperation {
    UpsertArtifact {
        kind: ArtifactKind,
        project_handle: String,
        key: String,
        content_type: String,
        artifact_id: String,
    },
    DeleteProject {
        project_handle: String,
    },
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

#[derive(Clone)]
struct Store {
    db: Arc<DB>,
    config: Arc<Config>,
}

impl Store {
    fn open(config: Arc<Config>) -> Result<Self, String> {
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        options.set_compression_type(rocksdb::DBCompressionType::Lz4);

        let cfs = vec![
            ColumnFamilyDescriptor::new(CF_MANIFESTS, Options::default()),
            ColumnFamilyDescriptor::new(CF_PROJECT_ARTIFACTS, Options::default()),
            ColumnFamilyDescriptor::new(CF_MULTIPART_UPLOADS, Options::default()),
            ColumnFamilyDescriptor::new(CF_OUTBOX, Options::default()),
        ];

        let db_path = config.data_dir.join("rocksdb");
        let db = DB::open_cf_descriptors(&options, db_path, cfs)
            .map_err(|error| format!("failed to open RocksDB: {error}"))?;

        Ok(Self {
            db: Arc::new(db),
            config,
        })
    }

    fn artifact_exists(
        &self,
        kind: ArtifactKind,
        project_handle: &str,
        key: &str,
    ) -> Result<bool, String> {
        let artifact_id = artifact_storage_id(kind, &self.config.tenant, project_handle, key);
        match self.manifest(&artifact_id)? {
            Some(manifest) => Ok(Path::new(&manifest.blob_path).exists()),
            None => Ok(false),
        }
    }

    fn manifest(&self, artifact_id: &str) -> Result<Option<ArtifactManifest>, String> {
        let raw = self
            .db
            .get_cf(self.cf(CF_MANIFESTS), artifact_id.as_bytes())
            .map_err(|error| format!("failed to read manifest: {error}"))?;

        raw.map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("failed to decode manifest: {error}"))
        })
        .transpose()
    }

    fn fetch_artifact(
        &self,
        kind: ArtifactKind,
        project_handle: &str,
        key: &str,
    ) -> Result<Option<ArtifactManifest>, String> {
        let artifact_id = artifact_storage_id(kind, &self.config.tenant, project_handle, key);
        match self.manifest(&artifact_id)? {
            Some(manifest) if Path::new(&manifest.blob_path).exists() => Ok(Some(manifest)),
            Some(_) => Ok(None),
            None => Ok(None),
        }
    }

    fn persist_artifact_from_path(
        &self,
        kind: ArtifactKind,
        project_handle: &str,
        key: &str,
        content_type: &str,
        source_path: &Path,
    ) -> Result<ArtifactManifest, String> {
        let artifact_id = artifact_storage_id(kind, &self.config.tenant, project_handle, key);
        let destination = blob_path(&self.config.data_dir, kind, &artifact_id);
        let parent = destination
            .parent()
            .ok_or_else(|| "missing blob parent directory".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create blob dir: {error}"))?;

        let size = std::fs::metadata(source_path)
            .map_err(|error| format!("failed to stat source blob: {error}"))?
            .len();

        if destination.exists() {
            let _ = std::fs::remove_file(source_path);
        } else if let Err(rename_error) = std::fs::rename(source_path, &destination) {
            std::fs::copy(source_path, &destination).map_err(|error| {
                format!("failed to copy blob after rename error ({rename_error}): {error}")
            })?;
            let _ = std::fs::remove_file(source_path);
        }

        let manifest = ArtifactManifest {
            artifact_id: artifact_id.clone(),
            kind,
            project_handle: project_handle.to_owned(),
            key: key.to_owned(),
            content_type: content_type.to_owned(),
            blob_path: destination.to_string_lossy().into_owned(),
            size,
            created_at_ms: now_ms(),
        };

        let mut batch = WriteBatch::default();
        let manifest_bytes = serde_json::to_vec(&manifest)
            .map_err(|error| format!("failed to encode manifest: {error}"))?;
        batch.put_cf(
            self.cf(CF_MANIFESTS),
            artifact_id.as_bytes(),
            manifest_bytes,
        );
        batch.put_cf(
            self.cf(CF_PROJECT_ARTIFACTS),
            project_artifact_index_key(project_handle, &artifact_id).as_bytes(),
            [],
        );

        self.db
            .write(batch)
            .map_err(|error| format!("failed to write manifest batch: {error}"))?;

        Ok(manifest)
    }

    fn persist_artifact_from_bytes(
        &self,
        kind: ArtifactKind,
        project_handle: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<ArtifactManifest, String> {
        let temp_path = temp_file_path(&self.config.tmp_dir.join("uploads"), "replication");
        std::fs::write(&temp_path, bytes)
            .map_err(|error| format!("failed to write temp blob: {error}"))?;
        self.persist_artifact_from_path(kind, project_handle, key, content_type, &temp_path)
    }

    fn delete_project(&self, project_handle: &str) -> Result<(), String> {
        let prefix = format!("{project_handle}\0");
        let mut batch = WriteBatch::default();
        let mut blob_paths = Vec::new();

        let iter = self.db.iterator_cf(
            self.cf(CF_PROJECT_ARTIFACTS),
            IteratorMode::From(prefix.as_bytes(), rocksdb::Direction::Forward),
        );

        for item in iter {
            let (index_key, _) =
                item.map_err(|error| format!("failed to iterate project index: {error}"))?;
            if !index_key.starts_with(prefix.as_bytes()) {
                break;
            }

            let artifact_id = std::str::from_utf8(&index_key[prefix.len()..])
                .map_err(|error| format!("invalid project index key: {error}"))?
                .to_owned();

            if let Some(manifest) = self.manifest(&artifact_id)? {
                blob_paths.push(PathBuf::from(manifest.blob_path));
            }

            batch.delete_cf(self.cf(CF_PROJECT_ARTIFACTS), index_key);
            batch.delete_cf(self.cf(CF_MANIFESTS), artifact_id.as_bytes());
        }

        self.db
            .write(batch)
            .map_err(|error| format!("failed to delete project batch: {error}"))?;

        for path in blob_paths {
            let _ = std::fs::remove_file(path);
        }

        Ok(())
    }

    fn start_multipart_upload(
        &self,
        account_handle: &str,
        project_handle: &str,
        category: &str,
        hash: &str,
        name: &str,
    ) -> Result<String, String> {
        let upload_id = Uuid::now_v7().to_string();
        let upload = MultipartUpload {
            upload_id: upload_id.clone(),
            account_handle: account_handle.to_owned(),
            project_handle: project_handle.to_owned(),
            category: category.to_owned(),
            hash: hash.to_owned(),
            name: name.to_owned(),
            parts: BTreeMap::new(),
            created_at_ms: now_ms(),
        };

        let upload_bytes = serde_json::to_vec(&upload)
            .map_err(|error| format!("failed to encode multipart upload: {error}"))?;
        self.db
            .put_cf(
                self.cf(CF_MULTIPART_UPLOADS),
                upload_id.as_bytes(),
                upload_bytes,
            )
            .map_err(|error| format!("failed to store multipart upload: {error}"))?;

        Ok(upload_id)
    }

    fn multipart_upload(&self, upload_id: &str) -> Result<Option<MultipartUpload>, String> {
        let raw = self
            .db
            .get_cf(self.cf(CF_MULTIPART_UPLOADS), upload_id.as_bytes())
            .map_err(|error| format!("failed to load multipart upload: {error}"))?;

        raw.map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("failed to decode multipart upload: {error}"))
        })
        .transpose()
    }

    fn add_multipart_part(
        &self,
        upload_id: &str,
        part_number: u32,
        part_path: &Path,
        size: u64,
    ) -> Result<(), MultipartError> {
        let mut upload = self
            .multipart_upload(upload_id)
            .map_err(MultipartError::Other)?
            .ok_or(MultipartError::NotFound)?;

        let current_total: u64 = upload.parts.values().map(|part| part.size).sum();
        let replaced_size = upload
            .parts
            .get(&part_number)
            .map(|part| part.size)
            .unwrap_or(0);
        let next_total = current_total - replaced_size + size;

        if next_total > MAX_MODULE_TOTAL_BYTES {
            return Err(MultipartError::TotalSizeExceeded);
        }

        let upload_dir = self.config.data_dir.join("multipart").join(upload_id);
        std::fs::create_dir_all(&upload_dir).map_err(|error| {
            MultipartError::Other(format!("failed to create multipart dir: {error}"))
        })?;
        let final_path = upload_dir.join(part_number.to_string());

        if let Err(rename_error) = std::fs::rename(part_path, &final_path) {
            std::fs::copy(part_path, &final_path).map_err(|error| {
                MultipartError::Other(format!(
                    "failed to store multipart part after rename error ({rename_error}): {error}"
                ))
            })?;
            let _ = std::fs::remove_file(part_path);
        }

        upload.parts.insert(
            part_number,
            MultipartPart {
                path: final_path.to_string_lossy().into_owned(),
                size,
            },
        );

        let upload_bytes = serde_json::to_vec(&upload).map_err(|error| {
            MultipartError::Other(format!("failed to encode multipart upload: {error}"))
        })?;
        self.db
            .put_cf(
                self.cf(CF_MULTIPART_UPLOADS),
                upload_id.as_bytes(),
                upload_bytes,
            )
            .map_err(|error| {
                MultipartError::Other(format!("failed to update multipart upload: {error}"))
            })?;

        Ok(())
    }

    fn complete_multipart_upload(
        &self,
        upload_id: &str,
        expected_parts: &[u32],
    ) -> Result<ArtifactManifest, MultipartError> {
        let upload = self
            .multipart_upload(upload_id)
            .map_err(MultipartError::Other)?
            .ok_or(MultipartError::NotFound)?;

        let uploaded: Vec<u32> = upload.parts.keys().copied().collect();
        if uploaded.is_empty() || uploaded != expected_parts {
            return Err(MultipartError::PartsMismatch);
        }

        let assembled_path = temp_file_path(&self.config.tmp_dir.join("uploads"), "module");
        let mut assembled = std::fs::File::create(&assembled_path).map_err(|error| {
            MultipartError::Other(format!("failed to create assembled artifact: {error}"))
        })?;

        for part_number in expected_parts {
            let part = upload
                .parts
                .get(part_number)
                .ok_or(MultipartError::PartsMismatch)?;
            let bytes = std::fs::read(&part.path).map_err(|error| {
                MultipartError::Other(format!("failed to read multipart part: {error}"))
            })?;
            use std::io::Write;
            assembled.write_all(&bytes).map_err(|error| {
                MultipartError::Other(format!("failed to assemble multipart artifact: {error}"))
            })?;
        }

        let key = module_key(&upload.category, &upload.hash, &upload.name);
        let manifest = self
            .persist_artifact_from_path(
                ArtifactKind::Module,
                &upload.project_handle,
                &key,
                "application/octet-stream",
                &assembled_path,
            )
            .map_err(MultipartError::Other)?;

        self.abort_multipart_upload(upload_id)
            .map_err(MultipartError::Other)?;

        Ok(manifest)
    }

    fn abort_multipart_upload(&self, upload_id: &str) -> Result<(), String> {
        if let Some(upload) = self.multipart_upload(upload_id)? {
            let _ = std::fs::remove_dir_all(self.config.data_dir.join("multipart").join(upload_id));
            self.db
                .delete_cf(self.cf(CF_MULTIPART_UPLOADS), upload_id.as_bytes())
                .map_err(|error| format!("failed to delete multipart upload: {error}"))?;

            for part in upload.parts.values() {
                let _ = std::fs::remove_file(&part.path);
            }
        }

        Ok(())
    }

    fn enqueue(&self, message: OutboxMessage) -> Result<(), String> {
        let key = format!("{:020}-{}", now_ms(), Uuid::now_v7());
        let value = serde_json::to_vec(&message)
            .map_err(|error| format!("failed to encode outbox message: {error}"))?;
        self.db
            .put_cf(self.cf(CF_OUTBOX), key.as_bytes(), value)
            .map_err(|error| format!("failed to enqueue outbox message: {error}"))
    }

    fn outbox_messages(&self) -> Result<Vec<(Vec<u8>, OutboxMessage)>, String> {
        let mut messages = Vec::new();
        let iter = self.db.iterator_cf(self.cf(CF_OUTBOX), IteratorMode::Start);
        for item in iter {
            let (key, value) =
                item.map_err(|error| format!("failed to iterate outbox: {error}"))?;
            let message = serde_json::from_slice::<OutboxMessage>(&value)
                .map_err(|error| format!("failed to decode outbox message: {error}"))?;
            messages.push((key.to_vec(), message));
        }
        Ok(messages)
    }

    fn delete_outbox_message(&self, key: &[u8]) -> Result<(), String> {
        self.db
            .delete_cf(self.cf(CF_OUTBOX), key)
            .map_err(|error| format!("failed to delete outbox entry: {error}"))
    }

    fn cf(&self, name: &str) -> &ColumnFamily {
        self.db
            .cf_handle(name)
            .expect("missing RocksDB column family")
    }
}

#[derive(Debug)]
enum MultipartError {
    NotFound,
    TotalSizeExceeded,
    PartsMismatch,
    Other(String),
}

#[derive(Clone)]
struct Metrics {
    registry: Arc<Mutex<Registry>>,
    http_requests: Family<HttpRequestLabels, Counter>,
    http_request_duration: Family<HttpRouteLabels, Histogram>,
    http_exceptions: Family<HttpExceptionLabels, Counter>,
    artifact_reads: Family<ArtifactOpLabels, Counter>,
    artifact_writes: Family<ArtifactOpLabels, Counter>,
    artifact_read_bytes: Family<ArtifactOpLabels, Counter>,
    artifact_write_bytes: Family<ArtifactOpLabels, Counter>,
    replication_requests: Family<ReplicationLabels, Counter>,
    replication_request_duration: Family<ReplicationRouteLabels, Histogram>,
    multipart_parts: Family<MultipartLabels, Counter>,
    node_info: Family<NodeInfoLabels, Gauge>,
}

impl Metrics {
    fn new(region: String, tenant: String) -> Self {
        let mut registry = Registry::default();

        let http_requests = Family::<HttpRequestLabels, Counter>::default();
        let http_request_duration =
            Family::<HttpRouteLabels, Histogram>::new_with_constructor(|| {
                Histogram::new(exponential_buckets(0.001, 2.0, 16))
            });
        let http_exceptions = Family::<HttpExceptionLabels, Counter>::default();
        let artifact_reads = Family::<ArtifactOpLabels, Counter>::default();
        let artifact_writes = Family::<ArtifactOpLabels, Counter>::default();
        let artifact_read_bytes = Family::<ArtifactOpLabels, Counter>::default();
        let artifact_write_bytes = Family::<ArtifactOpLabels, Counter>::default();
        let replication_requests = Family::<ReplicationLabels, Counter>::default();
        let replication_request_duration =
            Family::<ReplicationRouteLabels, Histogram>::new_with_constructor(|| {
                Histogram::new(exponential_buckets(0.001, 2.0, 16))
            });
        let multipart_parts = Family::<MultipartLabels, Counter>::default();
        let node_info = Family::<NodeInfoLabels, Gauge>::default();

        registry.register(
            "cache_http_requests_total",
            "HTTP requests by route and status code",
            http_requests.clone(),
        );
        registry.register(
            "cache_http_request_duration_seconds",
            "HTTP request latency by route",
            http_request_duration.clone(),
        );
        registry.register(
            "cache_http_exceptions_total",
            "HTTP exceptions by route and class",
            http_exceptions.clone(),
        );
        registry.register(
            "cache_artifact_reads_total",
            "Artifact reads by kind and result",
            artifact_reads.clone(),
        );
        registry.register(
            "cache_artifact_writes_total",
            "Artifact writes by kind and result",
            artifact_writes.clone(),
        );
        registry.register(
            "cache_artifact_read_bytes_total",
            "Artifact read throughput by kind and result",
            artifact_read_bytes.clone(),
        );
        registry.register(
            "cache_artifact_write_bytes_total",
            "Artifact write throughput by kind and result",
            artifact_write_bytes.clone(),
        );
        registry.register(
            "cache_replication_requests_total",
            "Peer replication requests by target, operation, and result",
            replication_requests.clone(),
        );
        registry.register(
            "cache_replication_request_duration_seconds",
            "Peer replication request latency by target and operation",
            replication_request_duration.clone(),
        );
        registry.register(
            "cache_multipart_parts_total",
            "Multipart part uploads by result",
            multipart_parts.clone(),
        );
        registry.register(
            "cache_node_info",
            "Node info labels for each cache region",
            node_info.clone(),
        );

        let metrics = Self {
            registry: Arc::new(Mutex::new(registry)),
            http_requests,
            http_request_duration,
            http_exceptions,
            artifact_reads,
            artifact_writes,
            artifact_read_bytes,
            artifact_write_bytes,
            replication_requests,
            replication_request_duration,
            multipart_parts,
            node_info,
        };

        metrics
            .node_info
            .get_or_create(&NodeInfoLabels { region, tenant })
            .set(1);

        metrics
    }

    fn record_http(&self, route: String, method: String, status: StatusCode, duration: Duration) {
        self.http_requests
            .get_or_create(&HttpRequestLabels {
                route: route.clone(),
                method,
                status: status.as_u16(),
            })
            .inc();
        self.http_request_duration
            .get_or_create(&HttpRouteLabels {
                route: route.clone(),
            })
            .observe(duration.as_secs_f64());

        if status.is_server_error() {
            self.http_exceptions
                .get_or_create(&HttpExceptionLabels {
                    route,
                    kind: "server_error".into(),
                })
                .inc();
        }
    }

    fn record_artifact_read(&self, kind: ArtifactKind, result: &str, bytes: u64) {
        let labels = ArtifactOpLabels {
            kind: kind.as_str().to_owned(),
            result: result.to_owned(),
        };
        self.artifact_reads.get_or_create(&labels).inc();
        if bytes > 0 {
            self.artifact_read_bytes
                .get_or_create(&labels)
                .inc_by(bytes);
        }
    }

    fn record_artifact_write(&self, kind: ArtifactKind, result: &str, bytes: u64) {
        let labels = ArtifactOpLabels {
            kind: kind.as_str().to_owned(),
            result: result.to_owned(),
        };
        self.artifact_writes.get_or_create(&labels).inc();
        if bytes > 0 {
            self.artifact_write_bytes
                .get_or_create(&labels)
                .inc_by(bytes);
        }
    }

    fn record_replication(&self, target: &str, operation: &str, result: &str, duration: Duration) {
        self.replication_requests
            .get_or_create(&ReplicationLabels {
                target: replication_target_label(target),
                operation: operation.to_owned(),
                result: result.to_owned(),
            })
            .inc();
        self.replication_request_duration
            .get_or_create(&ReplicationRouteLabels {
                target: replication_target_label(target),
                operation: operation.to_owned(),
            })
            .observe(duration.as_secs_f64());
    }

    fn record_multipart_part(&self, result: &str) {
        self.multipart_parts
            .get_or_create(&MultipartLabels {
                result: result.to_owned(),
            })
            .inc();
    }

    fn render(&self) -> String {
        let mut encoded = String::new();
        let registry = self.registry.lock().expect("metrics registry poisoned");
        encode(&mut encoded, &registry).expect("failed to encode metrics");
        encoded
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct HttpRequestLabels {
    route: String,
    method: String,
    status: u16,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct HttpRouteLabels {
    route: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct HttpExceptionLabels {
    route: String,
    kind: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ArtifactOpLabels {
    kind: String,
    result: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ReplicationLabels {
    target: String,
    operation: String,
    result: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ReplicationRouteLabels {
    target: String,
    operation: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct MultipartLabels {
    result: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct NodeInfoLabels {
    region: String,
    tenant: String,
}

struct RequestHeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for RequestHeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|name| name.as_str()).collect()
    }
}

struct ReqwestHeaderInjector<'a>(&'a mut reqwest::header::HeaderMap);

impl Injector for ReqwestHeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_bytes()) else {
            return;
        };
        let Ok(value) = reqwest::header::HeaderValue::from_str(&value) else {
            return;
        };
        self.0.insert(name, value);
    }
}

fn inject_current_trace_context(headers: &mut reqwest::header::HeaderMap) {
    let context = Span::current().context();
    global::get_text_map_propagator(|propagator| {
        let mut injector = ReqwestHeaderInjector(headers);
        propagator.inject_context(&context, &mut injector);
    });
}

async fn track_http_metrics(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let start = std::time::Instant::now();
    let parent_context = global::get_text_map_propagator(|propagator| {
        propagator.extract(&RequestHeaderExtractor(req.headers()))
    });
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
    if let Err(error) = request_span.set_parent(parent_context) {
        warn!("failed to attach propagated trace context: {error:?}");
    }

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

async fn up(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let members = state.members.read().await.clone();
    let mut all_members = members;
    all_members.insert(state.config.region.clone());

    Json(serde_json::json!({
        "status": "ok",
        "tenant": state.config.tenant,
        "region": state.config.region,
        "node": state.config.region,
        "connected_nodes": all_members.iter().cloned().filter(|region| region != &state.config.region).collect::<Vec<_>>(),
        "ring_members": all_members.len(),
        "members": all_members.into_iter().collect::<Vec<_>>(),
    }))
}

async fn metrics_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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
    State(state): State<Arc<AppState>>,
) -> Response {
    let query = match required_query(&params, &["account_handle", "project_handle"]) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state
        .store
        .fetch_artifact(ArtifactKind::Keyvalue, &query.project_handle, &cas_id)
    {
        Ok(Some(manifest)) => {
            state
                .metrics
                .record_artifact_read(ArtifactKind::Keyvalue, "ok", manifest.size);
            serve_file(StatusCode::OK, &manifest).await
        }
        Ok(None) => {
            state
                .metrics
                .record_artifact_read(ArtifactKind::Keyvalue, "not_found", 0);
            error_response(
                StatusCode::NOT_FOUND,
                format!("No entries found for CAS ID {cas_id}."),
            )
        }
        Err(error) => {
            state
                .metrics
                .record_artifact_read(ArtifactKind::Keyvalue, "error", 0);
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Failed to fetch key-value entry: {error}"),
            )
        }
    }
}

async fn put_keyvalue(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<KeyValuePutRequest>,
) -> Response {
    let query = match required_query(&params, &["account_handle", "project_handle"]) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let payload = serde_json::json!({
        "cas_id": body.cas_id,
        "entries": body.entries.into_iter().map(|entry| serde_json::json!({ "value": entry.value })).collect::<Vec<_>>()
    });

    match state.store.persist_artifact_from_bytes(
        ArtifactKind::Keyvalue,
        &query.project_handle,
        payload["cas_id"].as_str().unwrap_or_default(),
        "application/json",
        payload.to_string().as_bytes(),
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
    State(state): State<Arc<AppState>>,
) -> Response {
    get_artifact(state, ArtifactKind::Xcode, &id, params).await
}

async fn put_xcode(
    AxumPath(id): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Response {
    put_blob_artifact(
        state,
        ArtifactKind::Xcode,
        id,
        params,
        request,
        MAX_XCODE_BYTES,
        StatusCode::NO_CONTENT,
    )
    .await
}

async fn get_gradle(
    AxumPath(cache_key): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> Response {
    get_artifact(state, ArtifactKind::Gradle, &cache_key, params).await
}

async fn put_gradle(
    AxumPath(cache_key): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Response {
    put_blob_artifact(
        state,
        ArtifactKind::Gradle,
        cache_key,
        params,
        request,
        MAX_GRADLE_BYTES,
        StatusCode::CREATED,
    )
    .await
}

async fn head_module(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let query = match required_query(
        &params,
        &["account_handle", "project_handle", "hash", "name"],
    ) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let key = module_key(
        params
            .get("cache_category")
            .map(String::as_str)
            .unwrap_or("builds"),
        &query.hash,
        &query.name,
    );

    match state
        .store
        .artifact_exists(ArtifactKind::Module, &query.project_handle, &key)
    {
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
    State(state): State<Arc<AppState>>,
) -> Response {
    let query = match required_query(
        &params,
        &["account_handle", "project_handle", "hash", "name"],
    ) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let key = module_key(
        params
            .get("cache_category")
            .map(String::as_str)
            .unwrap_or("builds"),
        &query.hash,
        &query.name,
    );
    get_artifact(state, ArtifactKind::Module, &key, params).await
}

async fn start_module_upload(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let query = match required_query(
        &params,
        &["account_handle", "project_handle", "hash", "name"],
    ) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let category = params
        .get("cache_category")
        .map(String::as_str)
        .unwrap_or("builds");
    let key = module_key(category, &query.hash, &query.name);

    match state
        .store
        .artifact_exists(ArtifactKind::Module, &query.project_handle, &key)
    {
        Ok(true) => {
            Json(serde_json::json!({ "upload_id": serde_json::Value::Null })).into_response()
        }
        Ok(false) => match state.store.start_multipart_upload(
            &query.account_handle,
            &query.project_handle,
            category,
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
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Response {
    let upload_id = match params.get("upload_id") {
        Some(value) => value.clone(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing upload_id"),
    };
    let part_number = match params
        .get("part_number")
        .and_then(|value| value.parse::<u32>().ok())
    {
        Some(part_number) => part_number,
        None => return error_response(StatusCode::BAD_REQUEST, "Invalid part_number"),
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
        .add_multipart_part(&upload_id, part_number, &temp.path, temp.size)
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
    State(state): State<Arc<AppState>>,
    Json(body): Json<CompleteMultipartRequest>,
) -> Response {
    let upload_id = match params.get("upload_id") {
        Some(value) => value.clone(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing upload_id"),
    };

    match state
        .store
        .complete_multipart_upload(&upload_id, &body.parts)
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

async fn clean_project(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let query = match required_query(&params, &["account_handle", "project_handle"]) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state.store.delete_project(&query.project_handle) {
        Ok(()) => {
            for peer in state
                .config
                .peers
                .iter()
                .filter(|peer| *peer != &state.config.node_url)
            {
                if let Err(error) = state.store.enqueue(OutboxMessage {
                    target: peer.clone(),
                    operation: ReplicationOperation::DeleteProject {
                        project_handle: query.project_handle.clone(),
                    },
                }) {
                    warn!("failed to enqueue project delete for {peer}: {error}");
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

async fn internal_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "region": state.config.region,
        "tenant": state.config.tenant,
    }))
}

async fn internal_replicate_artifact(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Response {
    let query = match required_query(&params, &["kind", "project_handle", "key", "content_type"]) {
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
        &query.project_handle,
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

async fn internal_delete_project(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let project_handle = match params.get("project_handle") {
        Some(value) => value,
        None => return error_response(StatusCode::BAD_REQUEST, "Missing project_handle"),
    };

    match state.store.delete_project(project_handle) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to delete replicated project: {error}"),
        ),
    }
}

async fn get_artifact(
    state: Arc<AppState>,
    kind: ArtifactKind,
    key: &str,
    params: HashMap<String, String>,
) -> Response {
    let query = match required_query(&params, &["account_handle", "project_handle"]) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state.store.fetch_artifact(kind, &query.project_handle, key) {
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
    state: Arc<AppState>,
    kind: ArtifactKind,
    key: String,
    params: HashMap<String, String>,
    request: Request,
    max_bytes: u64,
    success_status: StatusCode,
) -> Response {
    let query = match required_query(&params, &["account_handle", "project_handle"]) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    match state
        .store
        .artifact_exists(kind, &query.project_handle, &key)
    {
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
        &query.project_handle,
        &key,
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

fn enqueue_replication_for_artifact(state: &Arc<AppState>, manifest: &ArtifactManifest) {
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

#[derive(Debug)]
struct RequiredQuery {
    account_handle: String,
    project_handle: String,
    hash: String,
    name: String,
    kind: String,
    key: String,
    content_type: String,
}

fn required_query(
    params: &HashMap<String, String>,
    required: &[&str],
) -> Result<RequiredQuery, String> {
    for key in required {
        if !params.contains_key(*key) {
            return Err(format!("Missing {key}"));
        }
    }

    Ok(RequiredQuery {
        account_handle: params.get("account_handle").cloned().unwrap_or_default(),
        project_handle: params.get("project_handle").cloned().unwrap_or_default(),
        hash: params.get("hash").cloned().unwrap_or_default(),
        name: params.get("name").cloned().unwrap_or_default(),
        kind: params.get("kind").cloned().unwrap_or_default(),
        key: params.get("key").cloned().unwrap_or_default(),
        content_type: params.get("content_type").cloned().unwrap_or_default(),
    })
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = Json(serde_json::json!({ "message": message.into() }));
    (status, body).into_response()
}

struct TempBodyFile {
    path: PathBuf,
    size: u64,
}

enum BodyReadError {
    TooLarge,
    Io(String),
}

async fn read_request_to_temp(
    request: Request,
    directory: &Path,
    max_bytes: u64,
) -> Result<TempBodyFile, BodyReadError> {
    let temp_path = temp_file_path(directory, "upload");
    if let Some(parent) = temp_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|error| BodyReadError::Io(format!("failed to create temp dir: {error}")))?;
    }

    let mut file = fs::File::create(&temp_path)
        .await
        .map_err(|error| BodyReadError::Io(format!("failed to create temp file: {error}")))?;
    let mut stream = request.into_body().into_data_stream();
    let mut size = 0_u64;

    while let Some(item) = stream.next().await {
        let chunk = item
            .map_err(|error| BodyReadError::Io(format!("failed to read request body: {error}")))?;
        size += chunk.len() as u64;
        if size > max_bytes {
            let _ = fs::remove_file(&temp_path).await;
            return Err(BodyReadError::TooLarge);
        }

        file.write_all(&chunk)
            .await
            .map_err(|error| BodyReadError::Io(format!("failed to write temp file: {error}")))?;
    }

    file.flush()
        .await
        .map_err(|error| BodyReadError::Io(format!("failed to flush temp file: {error}")))?;

    Ok(TempBodyFile {
        path: temp_path,
        size,
    })
}

fn temp_file_path(directory: &Path, prefix: &str) -> PathBuf {
    directory.join(format!("{prefix}-{}", Uuid::now_v7()))
}

fn artifact_storage_id(
    kind: ArtifactKind,
    tenant: &str,
    project_handle: &str,
    key: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(tenant.as_bytes());
    hasher.update([0]);
    hasher.update(project_handle.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

fn blob_path(data_dir: &Path, kind: ArtifactKind, artifact_id: &str) -> PathBuf {
    data_dir
        .join("blobs")
        .join(kind.as_str())
        .join(&artifact_id[0..2])
        .join(&artifact_id[2..4])
        .join(artifact_id)
}

fn project_artifact_index_key(project_handle: &str, artifact_id: &str) -> String {
    format!("{project_handle}\0{artifact_id}")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}

fn module_key(category: &str, hash: &str, name: &str) -> String {
    format!("{category}/{hash}/{name}")
}

fn spawn_membership_task(state: Arc<AppState>) {
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

fn spawn_outbox_task(state: Arc<AppState>) {
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

async fn process_outbox(state: &Arc<AppState>) -> Result<(), String> {
    for (message_key, message) in state.store.outbox_messages()? {
        let started_at = std::time::Instant::now();
        let (operation_name, result) = match &message.operation {
            ReplicationOperation::UpsertArtifact {
                kind,
                project_handle,
                key: artifact_key,
                content_type,
                artifact_id,
            } => {
                let manifest = match state.store.manifest(artifact_id)? {
                    Some(manifest) => manifest,
                    None => {
                        state.store.delete_outbox_message(&message_key)?;
                        continue;
                    }
                };

                let file = match fs::File::open(&manifest.blob_path).await {
                    Ok(file) => file,
                    Err(error) => {
                        warn!(
                            "failed to open local blob for replication {}: {error}",
                            manifest.blob_path
                        );
                        continue;
                    }
                };

                let url = format!(
                    "{}/_internal/replicate/artifact?kind={}&project_handle={}&key={}&content_type={}",
                    message.target,
                    kind.as_str(),
                    url_encode(project_handle),
                    url_encode(artifact_key),
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
                (
                    "upsert_artifact",
                    async {
                        let mut headers = reqwest::header::HeaderMap::new();
                        inject_current_trace_context(&mut headers);
                        headers.insert(
                            reqwest::header::CONTENT_TYPE,
                            reqwest::header::HeaderValue::from_static("application/octet-stream"),
                        );

                        let response = state
                            .client
                            .put(&url)
                            .headers(headers)
                            .body(body)
                            .send()
                            .await
                            .map_err(|error| {
                                format!("artifact replication request failed: {error}")
                            })?;
                        response_span
                            .record("http.response.status_code", response.status().as_u16());
                        if response.status().is_server_error() {
                            response_span.record("otel.status_code", "ERROR");
                        }
                        response.error_for_status().map(|_| ()).map_err(|error| {
                            format!("artifact replication response failed: {error}")
                        })
                    }
                    .instrument(request_span)
                    .await,
                )
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
                (
                    "delete_project",
                    async {
                        let mut headers = reqwest::header::HeaderMap::new();
                        inject_current_trace_context(&mut headers);
                        let response = state
                            .client
                            .delete(&url)
                            .headers(headers)
                            .send()
                            .await
                            .map_err(|error| {
                                format!("project replication request failed: {error}")
                            })?;
                        response_span
                            .record("http.response.status_code", response.status().as_u16());
                        if response.status().is_server_error() {
                            response_span.record("otel.status_code", "ERROR");
                        }
                        response.error_for_status().map(|_| ()).map_err(|error| {
                            format!("project replication response failed: {error}")
                        })
                    }
                    .instrument(request_span)
                    .await,
                )
            }
        };

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

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{:02X}", other).chars().collect(),
        })
        .collect()
}

fn replication_target_label(value: &str) -> String {
    value
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(value)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_ids_are_stable() {
        let a = artifact_storage_id(ArtifactKind::Xcode, "tenant", "ios", "abc");
        let b = artifact_storage_id(ArtifactKind::Xcode, "tenant", "ios", "abc");
        let c = artifact_storage_id(ArtifactKind::Gradle, "tenant", "ios", "abc");

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn module_keys_include_category_hash_and_name() {
        assert_eq!(
            module_key("builds", "hash-1", "Module.framework"),
            "builds/hash-1/Module.framework"
        );
    }

    #[test]
    fn url_encoding_is_query_safe() {
        assert_eq!(
            url_encode("builds/hash 1/Module.framework"),
            "builds%2Fhash%201%2FModule.framework"
        );
    }
}
