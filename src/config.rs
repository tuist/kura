use std::path::PathBuf;

use tokio::fs;

const TUIST_CACHE_PORT: &str = "TUIST_CACHE_PORT";
const TUIST_CACHE_ACCOUNT_HANDLE: &str = "TUIST_CACHE_ACCOUNT_HANDLE";
const TUIST_CACHE_REGION: &str = "TUIST_CACHE_REGION";
const TUIST_CACHE_TMP_DIR: &str = "TUIST_CACHE_TMP_DIR";
const TUIST_CACHE_DATA_DIR: &str = "TUIST_CACHE_DATA_DIR";
const TUIST_CACHE_NODE_URL: &str = "TUIST_CACHE_NODE_URL";
const TUIST_CACHE_PEERS: &str = "TUIST_CACHE_PEERS";
const TUIST_CACHE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: &str =
    "TUIST_CACHE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";
const TUIST_CACHE_OTEL_SERVICE_NAME: &str = "TUIST_CACHE_OTEL_SERVICE_NAME";
const TUIST_CACHE_OTEL_DEPLOYMENT_ENVIRONMENT: &str = "TUIST_CACHE_OTEL_DEPLOYMENT_ENVIRONMENT";

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub account: String,
    pub region: String,
    pub tmp_dir: PathBuf,
    pub data_dir: PathBuf,
    pub node_url: String,
    pub peers: Vec<String>,
    pub otlp_traces_endpoint: String,
    pub otel_service_name: String,
    pub otel_deployment_environment: String,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub(crate) fn from_lookup<F>(mut lookup: F) -> Result<Self, String>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut missing = Vec::new();
        let mut invalid = Vec::new();

        let port =
            required_value(&mut lookup, TUIST_CACHE_PORT, &mut missing).and_then(
                |value| match value.parse::<u16>() {
                    Ok(port) => Some(port),
                    Err(_) => {
                        invalid.push(format!("{TUIST_CACHE_PORT} must be a valid u16"));
                        None
                    }
                },
            );
        let account = required_value(&mut lookup, TUIST_CACHE_ACCOUNT_HANDLE, &mut missing);
        let region = required_value(&mut lookup, TUIST_CACHE_REGION, &mut missing);
        let tmp_dir =
            required_value(&mut lookup, TUIST_CACHE_TMP_DIR, &mut missing).map(PathBuf::from);
        let data_dir =
            required_value(&mut lookup, TUIST_CACHE_DATA_DIR, &mut missing).map(PathBuf::from);
        let node_url = required_value(&mut lookup, TUIST_CACHE_NODE_URL, &mut missing);
        let peers = required_value(&mut lookup, TUIST_CACHE_PEERS, &mut missing).map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        });
        let otlp_traces_endpoint = required_value(
            &mut lookup,
            TUIST_CACHE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
            &mut missing,
        );
        let otel_service_name =
            required_value(&mut lookup, TUIST_CACHE_OTEL_SERVICE_NAME, &mut missing);
        let otel_deployment_environment = required_value(
            &mut lookup,
            TUIST_CACHE_OTEL_DEPLOYMENT_ENVIRONMENT,
            &mut missing,
        );

        if !missing.is_empty() || !invalid.is_empty() {
            let mut errors = Vec::new();
            if !missing.is_empty() {
                errors.push(format!(
                    "missing required environment variables: {}",
                    missing.join(", ")
                ));
            }
            errors.extend(invalid);
            return Err(errors.join("; "));
        }

        Ok(Self {
            port: port.expect("port should be present when configuration is valid"),
            account: account.expect("account should be present when configuration is valid"),
            region: region.expect("region should be present when configuration is valid"),
            tmp_dir: tmp_dir.expect("tmp_dir should be present when configuration is valid"),
            data_dir: data_dir.expect("data_dir should be present when configuration is valid"),
            node_url: node_url.expect("node_url should be present when configuration is valid"),
            peers: peers.expect("peers should be present when configuration is valid"),
            otlp_traces_endpoint: otlp_traces_endpoint
                .expect("otlp_traces_endpoint should be present when configuration is valid"),
            otel_service_name: otel_service_name
                .expect("otel_service_name should be present when configuration is valid"),
            otel_deployment_environment: otel_deployment_environment.expect(
                "otel_deployment_environment should be present when configuration is valid",
            ),
        })
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

fn required_value<F>(
    lookup: &mut F,
    key: &'static str,
    missing: &mut Vec<&'static str>,
) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    match lookup(key) {
        Some(value) => Some(value),
        None => {
            missing.push(key);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn config_from(values: &[(&str, &str)]) -> Result<Config, String> {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>();
        Config::from_lookup(|key| values.get(key).cloned())
    }

    #[test]
    fn from_lookup_reports_all_missing_variables() {
        let error = Config::from_lookup(|_| None).expect_err("expected missing config to fail");

        assert!(error.contains(TUIST_CACHE_PORT));
        assert!(error.contains(TUIST_CACHE_ACCOUNT_HANDLE));
        assert!(error.contains(TUIST_CACHE_REGION));
        assert!(error.contains(TUIST_CACHE_TMP_DIR));
        assert!(error.contains(TUIST_CACHE_DATA_DIR));
        assert!(error.contains(TUIST_CACHE_NODE_URL));
        assert!(error.contains(TUIST_CACHE_PEERS));
        assert!(error.contains(TUIST_CACHE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT));
        assert!(error.contains(TUIST_CACHE_OTEL_SERVICE_NAME));
        assert!(error.contains(TUIST_CACHE_OTEL_DEPLOYMENT_ENVIRONMENT));
    }

    #[test]
    fn from_lookup_parses_overrides() {
        let config = config_from(&[
            (TUIST_CACHE_PORT, "4500"),
            (TUIST_CACHE_ACCOUNT_HANDLE, "acme"),
            (TUIST_CACHE_REGION, "eu_west"),
            (TUIST_CACHE_TMP_DIR, "/tmp/cache"),
            (TUIST_CACHE_DATA_DIR, "/tmp/cache-data"),
            (TUIST_CACHE_NODE_URL, "https://cache.example.com"),
            (
                TUIST_CACHE_PEERS,
                "https://cache-a.example.com, https://cache-b.example.com",
            ),
            (
                TUIST_CACHE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (TUIST_CACHE_OTEL_SERVICE_NAME, "cache-eu"),
            (TUIST_CACHE_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect("expected config overrides to parse");

        assert_eq!(config.port, 4500);
        assert_eq!(config.account, "acme");
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
            config.otlp_traces_endpoint,
            "https://otel.example.com/v1/traces"
        );
        assert_eq!(config.otel_service_name, "cache-eu");
        assert_eq!(config.otel_deployment_environment, "staging");
    }

    #[test]
    fn from_lookup_reports_invalid_port() {
        let error = config_from(&[
            (TUIST_CACHE_PORT, "invalid"),
            (TUIST_CACHE_ACCOUNT_HANDLE, "acme"),
            (TUIST_CACHE_REGION, "eu_west"),
            (TUIST_CACHE_TMP_DIR, "/tmp/cache"),
            (TUIST_CACHE_DATA_DIR, "/tmp/cache-data"),
            (TUIST_CACHE_NODE_URL, "https://cache.example.com"),
            (TUIST_CACHE_PEERS, "https://cache-a.example.com"),
            (
                TUIST_CACHE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (TUIST_CACHE_OTEL_SERVICE_NAME, "cache-eu"),
            (TUIST_CACHE_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect_err("expected invalid port to fail");

        assert!(error.contains(TUIST_CACHE_PORT));
        assert!(error.contains("valid u16"));
    }

    #[tokio::test]
    async fn ensure_directories_creates_expected_layout() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut config = config_from(&[
            (TUIST_CACHE_PORT, "4000"),
            (TUIST_CACHE_ACCOUNT_HANDLE, "acme"),
            (TUIST_CACHE_REGION, "local"),
            (TUIST_CACHE_TMP_DIR, "/tmp/cache"),
            (TUIST_CACHE_DATA_DIR, "/tmp/cache-data"),
            (TUIST_CACHE_NODE_URL, "http://127.0.0.1:4000"),
            (TUIST_CACHE_PEERS, ""),
            (
                TUIST_CACHE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "http://127.0.0.1:4318/v1/traces",
            ),
            (TUIST_CACHE_OTEL_SERVICE_NAME, "cache-local"),
            (TUIST_CACHE_OTEL_DEPLOYMENT_ENVIRONMENT, "local"),
        ])
        .expect("expected config to parse");
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
