use std::path::PathBuf;

use tokio::fs;

const KURA_PORT: &str = "KURA_PORT";
const KURA_TENANT_ID: &str = "KURA_TENANT_ID";
const KURA_REGION: &str = "KURA_REGION";
const KURA_TMP_DIR: &str = "KURA_TMP_DIR";
const KURA_DATA_DIR: &str = "KURA_DATA_DIR";
const KURA_NODE_URL: &str = "KURA_NODE_URL";
const KURA_PEERS: &str = "KURA_PEERS";
const KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: &str = "KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";
const KURA_OTEL_SERVICE_NAME: &str = "KURA_OTEL_SERVICE_NAME";
const KURA_OTEL_DEPLOYMENT_ENVIRONMENT: &str = "KURA_OTEL_DEPLOYMENT_ENVIRONMENT";

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub tenant_id: String,
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
            required_value(&mut lookup, KURA_PORT, &mut missing).and_then(|value| {
                match value.parse::<u16>() {
                    Ok(port) => Some(port),
                    Err(_) => {
                        invalid.push(format!("{KURA_PORT} must be a valid u16"));
                        None
                    }
                }
            });
        let tenant_id = required_value(&mut lookup, KURA_TENANT_ID, &mut missing);
        let region = required_value(&mut lookup, KURA_REGION, &mut missing);
        let tmp_dir = required_value(&mut lookup, KURA_TMP_DIR, &mut missing).map(PathBuf::from);
        let data_dir = required_value(&mut lookup, KURA_DATA_DIR, &mut missing).map(PathBuf::from);
        let node_url = required_value(&mut lookup, KURA_NODE_URL, &mut missing);
        let peers = required_value(&mut lookup, KURA_PEERS, &mut missing).map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        });
        let otlp_traces_endpoint = required_value(
            &mut lookup,
            KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
            &mut missing,
        );
        let otel_service_name = required_value(&mut lookup, KURA_OTEL_SERVICE_NAME, &mut missing);
        let otel_deployment_environment =
            required_value(&mut lookup, KURA_OTEL_DEPLOYMENT_ENVIRONMENT, &mut missing);

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
            tenant_id: tenant_id.expect("tenant_id should be present when configuration is valid"),
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

        assert!(error.contains(KURA_PORT));
        assert!(error.contains(KURA_TENANT_ID));
        assert!(error.contains(KURA_REGION));
        assert!(error.contains(KURA_TMP_DIR));
        assert!(error.contains(KURA_DATA_DIR));
        assert!(error.contains(KURA_NODE_URL));
        assert!(error.contains(KURA_PEERS));
        assert!(error.contains(KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT));
        assert!(error.contains(KURA_OTEL_SERVICE_NAME));
        assert!(error.contains(KURA_OTEL_DEPLOYMENT_ENVIRONMENT));
    }

    #[test]
    fn from_lookup_parses_overrides() {
        let config = config_from(&[
            (KURA_PORT, "4500"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com"),
            (
                KURA_PEERS,
                "https://kura-a.example.com, https://kura-b.example.com",
            ),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect("expected config overrides to parse");

        assert_eq!(config.port, 4500);
        assert_eq!(config.tenant_id, "acme");
        assert_eq!(config.region, "eu_west");
        assert_eq!(config.tmp_dir, PathBuf::from("/tmp/kura"));
        assert_eq!(config.data_dir, PathBuf::from("/tmp/kura-data"));
        assert_eq!(config.node_url, "https://kura.example.com");
        assert_eq!(
            config.peers,
            vec![
                "https://kura-a.example.com".to_owned(),
                "https://kura-b.example.com".to_owned()
            ]
        );
        assert_eq!(
            config.otlp_traces_endpoint,
            "https://otel.example.com/v1/traces"
        );
        assert_eq!(config.otel_service_name, "kura-eu");
        assert_eq!(config.otel_deployment_environment, "staging");
    }

    #[test]
    fn from_lookup_reports_invalid_port() {
        let error = config_from(&[
            (KURA_PORT, "invalid"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com"),
            (KURA_PEERS, "https://kura-a.example.com"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect_err("expected invalid port to fail");

        assert!(error.contains(KURA_PORT));
        assert!(error.contains("valid u16"));
    }

    #[tokio::test]
    async fn ensure_directories_creates_expected_layout() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut config = config_from(&[
            (KURA_PORT, "4000"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "local"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "http://127.0.0.1:4000"),
            (KURA_PEERS, ""),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "http://127.0.0.1:4318/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-local"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "local"),
        ])
        .expect("expected config to parse");
        config.tmp_dir = temp_dir.path().join("tmp");
        config.data_dir = temp_dir.path().join("data");

        config
            .ensure_directories()
            .await
            .expect("failed to create Kura directories");

        assert!(config.tmp_dir.join("uploads").exists());
        assert!(config.tmp_dir.join("parts").exists());
        assert!(config.data_dir.join("rocksdb").exists());
        assert!(config.data_dir.join("blobs").exists());
        assert!(config.data_dir.join("multipart").exists());
    }
}
