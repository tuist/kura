use std::path::PathBuf;

use tokio::fs;

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub tenant: String,
    pub region: String,
    pub tmp_dir: PathBuf,
    pub data_dir: PathBuf,
    pub node_url: String,
    pub peers: Vec<String>,
    pub otlp_traces_endpoint: Option<String>,
    pub otel_service_name: String,
    pub otel_deployment_environment: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub(crate) fn from_lookup<F>(mut lookup: F) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        let port = lookup("PORT")
            .and_then(|value| value.parse().ok())
            .unwrap_or(4000);
        let tenant = lookup("TENANT_ID").unwrap_or_else(|| "demo-tenant".into());
        let region = lookup("CACHE_REGION").unwrap_or_else(|| "local".into());
        let tmp_dir = lookup("CACHE_TMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("tmp/cache"));
        let data_dir = lookup("CACHE_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("tmp/cache-data"));
        let node_url =
            lookup("CACHE_NODE_URL").unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
        let peers = lookup("CACHE_PEERS")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let otlp_traces_endpoint = lookup("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").or_else(|| {
            lookup("OTEL_EXPORTER_OTLP_ENDPOINT")
                .map(|value| format!("{}/v1/traces", value.trim_end_matches('/')))
        });
        let otel_service_name = lookup("OTEL_SERVICE_NAME")
            .unwrap_or_else(|| format!("cache-{}", region.replace('_', "-")));
        let otel_deployment_environment =
            lookup("OTEL_DEPLOYMENT_ENVIRONMENT").unwrap_or_else(|| "local".into());

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

    pub async fn ensure_directories(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(self.tmp_dir.join("uploads")).await?;
        fs::create_dir_all(self.tmp_dir.join("parts")).await?;
        fs::create_dir_all(self.data_dir.join("rocksdb")).await?;
        fs::create_dir_all(self.data_dir.join("blobs")).await?;
        fs::create_dir_all(self.data_dir.join("multipart")).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn config_from(values: &[(&str, &str)]) -> Config {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>();
        Config::from_lookup(|key| values.get(key).cloned())
    }

    #[test]
    fn from_lookup_uses_defaults() {
        let config = Config::from_lookup(|_| None);

        assert_eq!(config.port, 4000);
        assert_eq!(config.tenant, "demo-tenant");
        assert_eq!(config.region, "local");
        assert_eq!(config.tmp_dir, PathBuf::from("tmp/cache"));
        assert_eq!(config.data_dir, PathBuf::from("tmp/cache-data"));
        assert_eq!(config.node_url, "http://127.0.0.1:4000");
        assert!(config.peers.is_empty());
        assert_eq!(config.otel_service_name, "cache-local");
        assert_eq!(config.otel_deployment_environment, "local");
        assert!(config.otlp_traces_endpoint.is_none());
    }

    #[test]
    fn from_lookup_parses_overrides() {
        let config = config_from(&[
            ("PORT", "4500"),
            ("TENANT_ID", "acme"),
            ("CACHE_REGION", "eu_west"),
            ("CACHE_TMP_DIR", "/tmp/cache"),
            ("CACHE_DATA_DIR", "/tmp/cache-data"),
            ("CACHE_NODE_URL", "https://cache.example.com"),
            (
                "CACHE_PEERS",
                "https://cache-a.example.com, https://cache-b.example.com",
            ),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "https://otel.example.com"),
            ("OTEL_SERVICE_NAME", "cache-eu"),
            ("OTEL_DEPLOYMENT_ENVIRONMENT", "staging"),
        ]);

        assert_eq!(config.port, 4500);
        assert_eq!(config.tenant, "acme");
        assert_eq!(config.region, "eu_west");
        assert_eq!(config.tmp_dir, PathBuf::from("/tmp/cache"));
        assert_eq!(config.data_dir, PathBuf::from("/tmp/cache-data"));
        assert_eq!(config.node_url, "https://cache.example.com");
        assert_eq!(
            config.peers,
            vec![
                "https://cache-a.example.com".to_owned(),
                "https://cache-b.example.com".to_owned()
            ]
        );
        assert_eq!(
            config.otlp_traces_endpoint.as_deref(),
            Some("https://otel.example.com/v1/traces")
        );
        assert_eq!(config.otel_service_name, "cache-eu");
        assert_eq!(config.otel_deployment_environment, "staging");
    }

    #[tokio::test]
    async fn ensure_directories_creates_expected_layout() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut config = Config::from_lookup(|_| None);
        config.tmp_dir = temp_dir.path().join("tmp");
        config.data_dir = temp_dir.path().join("data");

        config
            .ensure_directories()
            .await
            .expect("failed to create cache directories");

        assert!(config.tmp_dir.join("uploads").exists());
        assert!(config.tmp_dir.join("parts").exists());
        assert!(config.data_dir.join("rocksdb").exists());
        assert!(config.data_dir.join("blobs").exists());
        assert!(config.data_dir.join("multipart").exists());
    }
}
