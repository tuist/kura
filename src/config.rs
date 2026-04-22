use std::path::PathBuf;

use tokio::fs;

const KURA_PORT: &str = "KURA_PORT";
const KURA_GRPC_PORT: &str = "KURA_GRPC_PORT";
const KURA_TENANT_ID: &str = "KURA_TENANT_ID";
const KURA_REGION: &str = "KURA_REGION";
const KURA_TMP_DIR: &str = "KURA_TMP_DIR";
const KURA_DATA_DIR: &str = "KURA_DATA_DIR";
const KURA_NODE_URL: &str = "KURA_NODE_URL";
const KURA_PEERS: &str = "KURA_PEERS";
const KURA_DISCOVERY_DNS_NAME: &str = "KURA_DISCOVERY_DNS_NAME";
const KURA_INTERNAL_PORT: &str = "KURA_INTERNAL_PORT";
const KURA_INTERNAL_TLS_CA_CERT_PATH: &str = "KURA_INTERNAL_TLS_CA_CERT_PATH";
const KURA_INTERNAL_TLS_CERT_PATH: &str = "KURA_INTERNAL_TLS_CERT_PATH";
const KURA_INTERNAL_TLS_KEY_PATH: &str = "KURA_INTERNAL_TLS_KEY_PATH";
const KURA_FILE_DESCRIPTOR_POOL_SIZE: &str = "KURA_FILE_DESCRIPTOR_POOL_SIZE";
const KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS: &str = "KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS";
const KURA_SEGMENT_HANDLE_CACHE_SIZE: &str = "KURA_SEGMENT_HANDLE_CACHE_SIZE";
const KURA_MEMORY_SOFT_LIMIT_BYTES: &str = "KURA_MEMORY_SOFT_LIMIT_BYTES";
const KURA_MEMORY_HARD_LIMIT_BYTES: &str = "KURA_MEMORY_HARD_LIMIT_BYTES";
const KURA_MANIFEST_CACHE_MAX_BYTES: &str = "KURA_MANIFEST_CACHE_MAX_BYTES";
const KURA_MAX_KEYVALUE_BYTES: &str = "KURA_MAX_KEYVALUE_BYTES";
const KURA_ROCKSDB_MAX_OPEN_FILES: &str = "KURA_ROCKSDB_MAX_OPEN_FILES";
const KURA_ROCKSDB_MAX_BACKGROUND_JOBS: &str = "KURA_ROCKSDB_MAX_BACKGROUND_JOBS";
const KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: &str = "KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";
const KURA_OTEL_SERVICE_NAME: &str = "KURA_OTEL_SERVICE_NAME";
const KURA_OTEL_DEPLOYMENT_ENVIRONMENT: &str = "KURA_OTEL_DEPLOYMENT_ENVIRONMENT";

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub grpc_port: u16,
    pub tenant_id: String,
    pub region: String,
    pub tmp_dir: PathBuf,
    pub data_dir: PathBuf,
    pub node_url: String,
    pub peers: Vec<String>,
    pub discovery_dns_name: Option<String>,
    pub peer_tls: Option<PeerTlsConfig>,
    pub file_descriptor_pool_size: usize,
    pub file_descriptor_acquire_timeout_ms: u64,
    pub segment_handle_cache_size: usize,
    pub memory_soft_limit_bytes: u64,
    pub memory_hard_limit_bytes: u64,
    pub manifest_cache_max_bytes: usize,
    pub max_keyvalue_bytes: usize,
    pub rocksdb_max_open_files: i32,
    pub rocksdb_max_background_jobs: i32,
    pub otlp_traces_endpoint: String,
    pub otel_service_name: String,
    pub otel_deployment_environment: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerTlsConfig {
    pub internal_port: u16,
    pub ca_cert_path: PathBuf,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
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
        let grpc_port =
            required_value(&mut lookup, KURA_GRPC_PORT, &mut missing).and_then(|value| match value
                .parse::<u16>(
            ) {
                Ok(port) => Some(port),
                Err(_) => {
                    invalid.push(format!("{KURA_GRPC_PORT} must be a valid u16"));
                    None
                }
            });
        let tenant_id = required_value(&mut lookup, KURA_TENANT_ID, &mut missing);
        let region = required_value(&mut lookup, KURA_REGION, &mut missing);
        let tmp_dir = required_value(&mut lookup, KURA_TMP_DIR, &mut missing).map(PathBuf::from);
        let data_dir = required_value(&mut lookup, KURA_DATA_DIR, &mut missing).map(PathBuf::from);
        let node_url = required_value(&mut lookup, KURA_NODE_URL, &mut missing);
        let peers: Option<Vec<String>> =
            required_value(&mut lookup, KURA_PEERS, &mut missing).map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            });
        let discovery_dns_name = lookup(KURA_DISCOVERY_DNS_NAME)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let internal_port = lookup(KURA_INTERNAL_PORT)
            .map(|value| {
                value
                    .parse::<u16>()
                    .map_err(|_| format!("{KURA_INTERNAL_PORT} must be a valid u16"))
            })
            .transpose()?;
        let internal_tls_ca_cert_path = lookup(KURA_INTERNAL_TLS_CA_CERT_PATH)
            .map(PathBuf::from)
            .filter(|value| !value.as_os_str().is_empty());
        let internal_tls_cert_path = lookup(KURA_INTERNAL_TLS_CERT_PATH)
            .map(PathBuf::from)
            .filter(|value| !value.as_os_str().is_empty());
        let internal_tls_key_path = lookup(KURA_INTERNAL_TLS_KEY_PATH)
            .map(PathBuf::from)
            .filter(|value| !value.as_os_str().is_empty());
        let peer_tls = match (
            internal_port,
            internal_tls_ca_cert_path,
            internal_tls_cert_path,
            internal_tls_key_path,
        ) {
            (None, None, None, None) => None,
            (Some(internal_port), Some(ca_cert_path), Some(cert_path), Some(key_path)) => {
                Some(PeerTlsConfig {
                    internal_port,
                    ca_cert_path,
                    cert_path,
                    key_path,
                })
            }
            _ => {
                invalid.push(format!(
                    "{KURA_INTERNAL_PORT}, {KURA_INTERNAL_TLS_CA_CERT_PATH}, {KURA_INTERNAL_TLS_CERT_PATH}, and {KURA_INTERNAL_TLS_KEY_PATH} must either all be set or all be unset"
                ));
                None
            }
        };
        let file_descriptor_pool_size =
            required_value(&mut lookup, KURA_FILE_DESCRIPTOR_POOL_SIZE, &mut missing).and_then(
                |value| match value.parse::<usize>() {
                    Ok(pool_size) if pool_size > 0 => Some(pool_size),
                    Ok(_) => {
                        invalid.push(format!(
                            "{KURA_FILE_DESCRIPTOR_POOL_SIZE} must be greater than 0"
                        ));
                        None
                    }
                    Err(_) => {
                        invalid.push(format!(
                            "{KURA_FILE_DESCRIPTOR_POOL_SIZE} must be a valid usize"
                        ));
                        None
                    }
                },
            );
        let file_descriptor_acquire_timeout_ms = required_value(
            &mut lookup,
            KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS,
            &mut missing,
        )
        .and_then(|value| match value.parse::<u64>() {
            Ok(timeout_ms) if timeout_ms > 0 => Some(timeout_ms),
            Ok(_) => {
                invalid.push(format!(
                    "{KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS} must be greater than 0"
                ));
                None
            }
            Err(_) => {
                invalid.push(format!(
                    "{KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS} must be a valid u64"
                ));
                None
            }
        });
        let segment_handle_cache_size =
            required_value(&mut lookup, KURA_SEGMENT_HANDLE_CACHE_SIZE, &mut missing).and_then(
                |value| match value.parse::<usize>() {
                    Ok(cache_size) => {
                        if let Some(pool_size) = file_descriptor_pool_size {
                            if cache_size >= pool_size {
                                invalid.push(format!(
                                    "{KURA_SEGMENT_HANDLE_CACHE_SIZE} must be less than {KURA_FILE_DESCRIPTOR_POOL_SIZE} so transient file operations keep headroom"
                                ));
                                None
                            } else {
                                Some(cache_size)
                            }
                        } else {
                            Some(cache_size)
                        }
                    }
                    Err(_) => {
                        invalid.push(format!(
                            "{KURA_SEGMENT_HANDLE_CACHE_SIZE} must be a valid usize"
                        ));
                        None
                    }
                },
            );
        let memory_soft_limit_bytes =
            required_value(&mut lookup, KURA_MEMORY_SOFT_LIMIT_BYTES, &mut missing).and_then(
                |value| match value.parse::<u64>() {
                    Ok(limit) if limit > 0 => Some(limit),
                    Ok(_) => {
                        invalid.push(format!(
                            "{KURA_MEMORY_SOFT_LIMIT_BYTES} must be greater than 0"
                        ));
                        None
                    }
                    Err(_) => {
                        invalid.push(format!(
                            "{KURA_MEMORY_SOFT_LIMIT_BYTES} must be a valid u64"
                        ));
                        None
                    }
                },
            );
        let memory_hard_limit_bytes =
            required_value(&mut lookup, KURA_MEMORY_HARD_LIMIT_BYTES, &mut missing).and_then(
                |value| match value.parse::<u64>() {
                    Ok(limit) => {
                        if let Some(soft_limit) = memory_soft_limit_bytes {
                            if limit <= soft_limit {
                                invalid.push(format!(
                                    "{KURA_MEMORY_HARD_LIMIT_BYTES} must be greater than {KURA_MEMORY_SOFT_LIMIT_BYTES}"
                                ));
                                None
                            } else {
                                Some(limit)
                            }
                        } else if limit > 0 {
                            Some(limit)
                        } else {
                            invalid.push(format!(
                                "{KURA_MEMORY_HARD_LIMIT_BYTES} must be greater than 0"
                            ));
                            None
                        }
                    }
                    Err(_) => {
                        invalid.push(format!(
                            "{KURA_MEMORY_HARD_LIMIT_BYTES} must be a valid u64"
                        ));
                        None
                    }
                },
            );
        let manifest_cache_max_bytes =
            required_value(&mut lookup, KURA_MANIFEST_CACHE_MAX_BYTES, &mut missing).and_then(
                |value| match value.parse::<usize>() {
                    Ok(limit) if limit > 0 => {
                        if let Some(soft_limit) = memory_soft_limit_bytes {
                            if limit as u64 >= soft_limit {
                                invalid.push(format!(
                                    "{KURA_MANIFEST_CACHE_MAX_BYTES} must be less than {KURA_MEMORY_SOFT_LIMIT_BYTES} so the cache leaves heap headroom"
                                ));
                                None
                            } else {
                                Some(limit)
                            }
                        } else {
                            Some(limit)
                        }
                    }
                    Ok(_) => {
                        invalid.push(format!(
                            "{KURA_MANIFEST_CACHE_MAX_BYTES} must be greater than 0"
                        ));
                        None
                    }
                    Err(_) => {
                        invalid.push(format!(
                            "{KURA_MANIFEST_CACHE_MAX_BYTES} must be a valid usize"
                        ));
                        None
                    }
                },
            );
        let max_keyvalue_bytes = required_value(&mut lookup, KURA_MAX_KEYVALUE_BYTES, &mut missing)
            .and_then(|value| match value.parse::<usize>() {
                Ok(limit) if limit > 0 => Some(limit),
                Ok(_) => {
                    invalid.push(format!("{KURA_MAX_KEYVALUE_BYTES} must be greater than 0"));
                    None
                }
                Err(_) => {
                    invalid.push(format!("{KURA_MAX_KEYVALUE_BYTES} must be a valid usize"));
                    None
                }
            });
        let rocksdb_max_open_files =
            required_value(&mut lookup, KURA_ROCKSDB_MAX_OPEN_FILES, &mut missing).and_then(
                |value| match value.parse::<i32>() {
                    Ok(max_open_files) if max_open_files > 0 || max_open_files == -1 => {
                        Some(max_open_files)
                    }
                    Ok(_) => {
                        invalid.push(format!(
                            "{KURA_ROCKSDB_MAX_OPEN_FILES} must be -1 or greater than 0"
                        ));
                        None
                    }
                    Err(_) => {
                        invalid.push(format!("{KURA_ROCKSDB_MAX_OPEN_FILES} must be a valid i32"));
                        None
                    }
                },
            );
        let rocksdb_max_background_jobs =
            required_value(&mut lookup, KURA_ROCKSDB_MAX_BACKGROUND_JOBS, &mut missing).and_then(
                |value| match value.parse::<i32>() {
                    Ok(max_background_jobs) if max_background_jobs > 0 => Some(max_background_jobs),
                    Ok(_) => {
                        invalid.push(format!(
                            "{KURA_ROCKSDB_MAX_BACKGROUND_JOBS} must be greater than 0"
                        ));
                        None
                    }
                    Err(_) => {
                        invalid.push(format!(
                            "{KURA_ROCKSDB_MAX_BACKGROUND_JOBS} must be a valid i32"
                        ));
                        None
                    }
                },
            );
        let otlp_traces_endpoint = required_value(
            &mut lookup,
            KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
            &mut missing,
        );
        let otel_service_name = required_value(&mut lookup, KURA_OTEL_SERVICE_NAME, &mut missing);
        let otel_deployment_environment =
            required_value(&mut lookup, KURA_OTEL_DEPLOYMENT_ENVIRONMENT, &mut missing);

        if let (Some(node_url), Some(peers), Some(peer_tls)) =
            (node_url.as_ref(), peers.as_ref(), peer_tls.as_ref())
        {
            if let Some(port) = port {
                if peer_tls.internal_port == port {
                    invalid.push(format!(
                        "{KURA_INTERNAL_PORT} must differ from {KURA_PORT} when peer mTLS is enabled"
                    ));
                }
            }
            match reqwest::Url::parse(node_url) {
                Ok(url) => {
                    let scheme = url.scheme();
                    let port = url.port_or_known_default();
                    if scheme != "https" {
                        invalid.push(format!(
                            "{KURA_NODE_URL} must use https when peer mTLS is enabled"
                        ));
                    }
                    if port != Some(peer_tls.internal_port) {
                        invalid.push(format!(
                            "{KURA_NODE_URL} must target port {} when peer mTLS is enabled",
                            peer_tls.internal_port
                        ));
                    }
                }
                Err(error) => invalid.push(format!("{KURA_NODE_URL} must be a valid URL: {error}")),
            }

            for peer in peers.iter().map(String::as_str) {
                match reqwest::Url::parse(peer) {
                    Ok(url) if url.scheme() == "https" => {}
                    Ok(_) => invalid.push(format!(
                        "peer URL {peer} must use https when peer mTLS is enabled"
                    )),
                    Err(error) => invalid.push(format!("peer URL {peer} must be valid: {error}")),
                }
            }
        }

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
            grpc_port: grpc_port.expect("grpc_port should be present when configuration is valid"),
            tenant_id: tenant_id.expect("tenant_id should be present when configuration is valid"),
            region: region.expect("region should be present when configuration is valid"),
            tmp_dir: tmp_dir.expect("tmp_dir should be present when configuration is valid"),
            data_dir: data_dir.expect("data_dir should be present when configuration is valid"),
            node_url: node_url.expect("node_url should be present when configuration is valid"),
            peers: peers.expect("peers should be present when configuration is valid"),
            discovery_dns_name,
            peer_tls,
            file_descriptor_pool_size: file_descriptor_pool_size
                .expect("file_descriptor_pool_size should be present when configuration is valid"),
            file_descriptor_acquire_timeout_ms: file_descriptor_acquire_timeout_ms.expect(
                "file_descriptor_acquire_timeout_ms should be present when configuration is valid",
            ),
            segment_handle_cache_size: segment_handle_cache_size
                .expect("segment_handle_cache_size should be present when configuration is valid"),
            memory_soft_limit_bytes: memory_soft_limit_bytes
                .expect("memory_soft_limit_bytes should be present when configuration is valid"),
            memory_hard_limit_bytes: memory_hard_limit_bytes
                .expect("memory_hard_limit_bytes should be present when configuration is valid"),
            manifest_cache_max_bytes: manifest_cache_max_bytes
                .expect("manifest_cache_max_bytes should be present when configuration is valid"),
            max_keyvalue_bytes: max_keyvalue_bytes
                .expect("max_keyvalue_bytes should be present when configuration is valid"),
            rocksdb_max_open_files: rocksdb_max_open_files
                .expect("rocksdb_max_open_files should be present when configuration is valid"),
            rocksdb_max_background_jobs: rocksdb_max_background_jobs.expect(
                "rocksdb_max_background_jobs should be present when configuration is valid",
            ),
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
        fs::create_dir_all(self.data_dir.join("segments")).await?;
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
        assert!(error.contains(KURA_GRPC_PORT));
        assert!(error.contains(KURA_TENANT_ID));
        assert!(error.contains(KURA_REGION));
        assert!(error.contains(KURA_TMP_DIR));
        assert!(error.contains(KURA_DATA_DIR));
        assert!(error.contains(KURA_NODE_URL));
        assert!(error.contains(KURA_PEERS));
        assert!(error.contains(KURA_FILE_DESCRIPTOR_POOL_SIZE));
        assert!(error.contains(KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS));
        assert!(error.contains(KURA_SEGMENT_HANDLE_CACHE_SIZE));
        assert!(error.contains(KURA_MEMORY_SOFT_LIMIT_BYTES));
        assert!(error.contains(KURA_MEMORY_HARD_LIMIT_BYTES));
        assert!(error.contains(KURA_MANIFEST_CACHE_MAX_BYTES));
        assert!(error.contains(KURA_MAX_KEYVALUE_BYTES));
        assert!(error.contains(KURA_ROCKSDB_MAX_OPEN_FILES));
        assert!(error.contains(KURA_ROCKSDB_MAX_BACKGROUND_JOBS));
        assert!(error.contains(KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT));
        assert!(error.contains(KURA_OTEL_SERVICE_NAME));
        assert!(error.contains(KURA_OTEL_DEPLOYMENT_ENVIRONMENT));
    }

    #[test]
    fn from_lookup_parses_overrides() {
        let config = config_from(&[
            (KURA_PORT, "4500"),
            (KURA_GRPC_PORT, "5500"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com"),
            (
                KURA_PEERS,
                "https://kura-a.example.com, https://kura-b.example.com",
            ),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "64"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "5000"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "16"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "268435456"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "536870912"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "16777216"),
            (KURA_MAX_KEYVALUE_BYTES, "1048576"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "1024"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "4"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect("expected config overrides to parse");

        assert_eq!(config.port, 4500);
        assert_eq!(config.grpc_port, 5500);
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
        assert_eq!(config.discovery_dns_name, None);
        assert_eq!(config.peer_tls, None);
        assert_eq!(config.file_descriptor_pool_size, 64);
        assert_eq!(config.file_descriptor_acquire_timeout_ms, 5000);
        assert_eq!(config.segment_handle_cache_size, 16);
        assert_eq!(config.memory_soft_limit_bytes, 268_435_456);
        assert_eq!(config.memory_hard_limit_bytes, 536_870_912);
        assert_eq!(config.manifest_cache_max_bytes, 16_777_216);
        assert_eq!(config.max_keyvalue_bytes, 1_048_576);
        assert_eq!(config.rocksdb_max_open_files, 1024);
        assert_eq!(config.rocksdb_max_background_jobs, 4);
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
            (KURA_GRPC_PORT, "invalid"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com"),
            (KURA_PEERS, "https://kura-a.example.com"),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "invalid"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "invalid"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "invalid"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "invalid"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "invalid"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "invalid"),
            (KURA_MAX_KEYVALUE_BYTES, "invalid"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "invalid"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "invalid"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect_err("expected invalid port to fail");

        assert!(error.contains(KURA_PORT));
        assert!(error.contains(KURA_GRPC_PORT));
        assert!(error.contains("valid u16"));
        assert!(error.contains(KURA_FILE_DESCRIPTOR_POOL_SIZE));
        assert!(error.contains(KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS));
        assert!(error.contains(KURA_SEGMENT_HANDLE_CACHE_SIZE));
        assert!(error.contains(KURA_MEMORY_SOFT_LIMIT_BYTES));
        assert!(error.contains(KURA_MEMORY_HARD_LIMIT_BYTES));
        assert!(error.contains(KURA_MANIFEST_CACHE_MAX_BYTES));
        assert!(error.contains(KURA_MAX_KEYVALUE_BYTES));
        assert!(error.contains(KURA_ROCKSDB_MAX_OPEN_FILES));
        assert!(error.contains(KURA_ROCKSDB_MAX_BACKGROUND_JOBS));
    }

    #[test]
    fn from_lookup_parses_optional_discovery_dns_name() {
        let config = config_from(&[
            (KURA_PORT, "4500"),
            (KURA_GRPC_PORT, "5500"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com"),
            (KURA_PEERS, "https://kura-a.example.com"),
            (KURA_DISCOVERY_DNS_NAME, "kura-ring.internal"),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "64"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "5000"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "16"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "268435456"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "536870912"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "16777216"),
            (KURA_MAX_KEYVALUE_BYTES, "1048576"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "1024"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "4"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect("expected config overrides to parse");

        assert_eq!(
            config.discovery_dns_name.as_deref(),
            Some("kura-ring.internal")
        );
    }

    #[test]
    fn from_lookup_requires_segment_handle_cache_headroom() {
        let error = config_from(&[
            (KURA_PORT, "4000"),
            (KURA_GRPC_PORT, "5000"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com"),
            (KURA_PEERS, "https://kura-a.example.com"),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "16"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "5000"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "16"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "268435456"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "536870912"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "16777216"),
            (KURA_MAX_KEYVALUE_BYTES, "1048576"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "128"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "2"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect_err("expected equal segment handle cache size to fail");

        assert!(error.contains(KURA_SEGMENT_HANDLE_CACHE_SIZE));
        assert!(error.contains(KURA_FILE_DESCRIPTOR_POOL_SIZE));
    }

    #[test]
    fn from_lookup_requires_manifest_cache_to_leave_memory_headroom() {
        let error = config_from(&[
            (KURA_PORT, "4000"),
            (KURA_GRPC_PORT, "5000"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com"),
            (KURA_PEERS, "https://kura-a.example.com"),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "16"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "5000"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "8"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "1048576"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "2097152"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "1048576"),
            (KURA_MAX_KEYVALUE_BYTES, "262144"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "128"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "2"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect_err("expected manifest cache size at soft limit to fail");

        assert!(error.contains(KURA_MANIFEST_CACHE_MAX_BYTES));
        assert!(error.contains(KURA_MEMORY_SOFT_LIMIT_BYTES));
    }

    #[test]
    fn from_lookup_parses_peer_tls_config() {
        let config = config_from(&[
            (KURA_PORT, "4500"),
            (KURA_GRPC_PORT, "5500"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com:7443"),
            (
                KURA_PEERS,
                "https://kura-a.example.com:7443, https://kura-b.example.com:7443",
            ),
            (KURA_INTERNAL_PORT, "7443"),
            (KURA_INTERNAL_TLS_CA_CERT_PATH, "/etc/kura/peer-ca.pem"),
            (KURA_INTERNAL_TLS_CERT_PATH, "/etc/kura/peer.pem"),
            (KURA_INTERNAL_TLS_KEY_PATH, "/etc/kura/peer.key"),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "64"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "5000"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "16"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "268435456"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "536870912"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "16777216"),
            (KURA_MAX_KEYVALUE_BYTES, "1048576"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "1024"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "4"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect("expected peer tls config to parse");

        assert_eq!(
            config.peer_tls,
            Some(PeerTlsConfig {
                internal_port: 7443,
                ca_cert_path: PathBuf::from("/etc/kura/peer-ca.pem"),
                cert_path: PathBuf::from("/etc/kura/peer.pem"),
                key_path: PathBuf::from("/etc/kura/peer.key"),
            })
        );
    }

    #[test]
    fn from_lookup_requires_complete_peer_tls_config() {
        let error = config_from(&[
            (KURA_PORT, "4500"),
            (KURA_GRPC_PORT, "5500"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "https://kura.example.com:7443"),
            (KURA_PEERS, "https://kura-a.example.com:7443"),
            (KURA_INTERNAL_PORT, "7443"),
            (KURA_INTERNAL_TLS_CA_CERT_PATH, "/etc/kura/peer-ca.pem"),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "64"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "5000"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "16"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "268435456"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "536870912"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "16777216"),
            (KURA_MAX_KEYVALUE_BYTES, "1048576"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "1024"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "4"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect_err("expected incomplete peer tls config to fail");

        assert!(error.contains(KURA_INTERNAL_PORT));
        assert!(error.contains(KURA_INTERNAL_TLS_CA_CERT_PATH));
        assert!(error.contains(KURA_INTERNAL_TLS_CERT_PATH));
        assert!(error.contains(KURA_INTERNAL_TLS_KEY_PATH));
    }

    #[test]
    fn from_lookup_requires_https_peer_urls_when_peer_tls_enabled() {
        let error = config_from(&[
            (KURA_PORT, "4500"),
            (KURA_GRPC_PORT, "5500"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "eu_west"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "http://kura.example.com:7443"),
            (KURA_PEERS, "http://kura-a.example.com:7443"),
            (KURA_INTERNAL_PORT, "7443"),
            (KURA_INTERNAL_TLS_CA_CERT_PATH, "/etc/kura/peer-ca.pem"),
            (KURA_INTERNAL_TLS_CERT_PATH, "/etc/kura/peer.pem"),
            (KURA_INTERNAL_TLS_KEY_PATH, "/etc/kura/peer.key"),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "64"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "5000"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "16"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "268435456"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "536870912"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "16777216"),
            (KURA_MAX_KEYVALUE_BYTES, "1048576"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "1024"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "4"),
            (
                KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "https://otel.example.com/v1/traces",
            ),
            (KURA_OTEL_SERVICE_NAME, "kura-eu"),
            (KURA_OTEL_DEPLOYMENT_ENVIRONMENT, "staging"),
        ])
        .expect_err("expected non-https peer urls to fail");

        assert!(error.contains(KURA_NODE_URL));
        assert!(error.contains("https"));
        assert!(error.contains("peer URL"));
    }

    #[tokio::test]
    async fn ensure_directories_creates_expected_layout() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut config = config_from(&[
            (KURA_PORT, "4000"),
            (KURA_GRPC_PORT, "5000"),
            (KURA_TENANT_ID, "acme"),
            (KURA_REGION, "local"),
            (KURA_TMP_DIR, "/tmp/kura"),
            (KURA_DATA_DIR, "/tmp/kura-data"),
            (KURA_NODE_URL, "http://127.0.0.1:4000"),
            (KURA_PEERS, ""),
            (KURA_FILE_DESCRIPTOR_POOL_SIZE, "32"),
            (KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS, "5000"),
            (KURA_SEGMENT_HANDLE_CACHE_SIZE, "8"),
            (KURA_MEMORY_SOFT_LIMIT_BYTES, "268435456"),
            (KURA_MEMORY_HARD_LIMIT_BYTES, "536870912"),
            (KURA_MANIFEST_CACHE_MAX_BYTES, "16777216"),
            (KURA_MAX_KEYVALUE_BYTES, "1048576"),
            (KURA_ROCKSDB_MAX_OPEN_FILES, "256"),
            (KURA_ROCKSDB_MAX_BACKGROUND_JOBS, "2"),
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
        assert!(config.data_dir.join("segments").exists());
        assert!(config.data_dir.join("multipart").exists());
    }
}
