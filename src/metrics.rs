use std::{sync::Mutex, time::Duration};

use axum::http::StatusCode;
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

use crate::{domain::ArtifactKind, utils::replication_target_label};

pub struct Metrics {
    registry: Mutex<Registry>,
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
    pub fn new(region: String, account: String) -> Self {
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
            registry: Mutex::new(registry),
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
            .get_or_create(&NodeInfoLabels { region, account })
            .set(1);

        metrics
    }

    pub fn record_http(
        &self,
        route: String,
        method: String,
        status: StatusCode,
        duration: Duration,
    ) {
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

    pub fn record_artifact_read(&self, kind: ArtifactKind, result: &str, bytes: u64) {
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

    pub fn record_artifact_write(&self, kind: ArtifactKind, result: &str, bytes: u64) {
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

    pub fn record_replication(
        &self,
        target: &str,
        operation: &str,
        result: &str,
        duration: Duration,
    ) {
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

    pub fn record_multipart_part(&self, result: &str) {
        self.multipart_parts
            .get_or_create(&MultipartLabels {
                result: result.to_owned(),
            })
            .inc();
    }

    pub fn render(&self) -> String {
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
    account: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_includes_recorded_metrics() {
        let metrics = Metrics::new("eu-west".into(), "acme".into());
        metrics.record_http(
            "/up".into(),
            "GET".into(),
            StatusCode::OK,
            Duration::from_millis(10),
        );
        metrics.record_http(
            "/api/cache/keyvalue".into(),
            "PUT".into(),
            StatusCode::INTERNAL_SERVER_ERROR,
            Duration::from_millis(20),
        );
        metrics.record_artifact_read(ArtifactKind::Xcode, "ok", 5);
        metrics.record_artifact_write(ArtifactKind::Module, "ok", 10);
        metrics.record_replication(
            "https://cache.example.com/internal",
            "upsert_artifact",
            "ok",
            Duration::from_millis(5),
        );
        metrics.record_multipart_part("ok");

        let rendered = metrics.render();

        assert!(rendered.contains("cache_http_requests_total"));
        assert!(rendered.contains("cache_http_exceptions_total"));
        assert!(rendered.contains("cache_artifact_reads_total"));
        assert!(rendered.contains("cache_artifact_write_bytes_total"));
        assert!(rendered.contains("cache_replication_requests_total"));
        assert!(rendered.contains("cache_multipart_parts_total"));
        assert!(rendered.contains("cache_node_info"));
        assert!(rendered.contains("region=\"eu-west\""));
        assert!(rendered.contains("account=\"acme\""));
    }
}
