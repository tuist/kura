use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use rocksdb::{ColumnFamily, ColumnFamilyDescriptor, DB, IteratorMode, Options, WriteBatch};
use uuid::Uuid;

use crate::{
    config::Config,
    constants::{
        CF_MANIFESTS, CF_MULTIPART_UPLOADS, CF_OUTBOX, CF_PROJECT_ARTIFACTS, MAX_MODULE_TOTAL_BYTES,
    },
    domain::{
        ArtifactKind, ArtifactManifest, MultipartError, MultipartPart, MultipartUpload,
        OutboxMessage,
    },
    utils::{
        artifact_storage_id, blob_path, module_key, now_ms, project_artifact_index_key,
        temp_file_path,
    },
};

pub struct Store {
    db: DB,
    account: String,
    tmp_dir: PathBuf,
    data_dir: PathBuf,
}

impl Store {
    pub fn open(config: &Config) -> Result<Self, String> {
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
            db,
            account: config.account.clone(),
            tmp_dir: config.tmp_dir.clone(),
            data_dir: config.data_dir.clone(),
        })
    }

    pub fn artifact_exists(
        &self,
        kind: ArtifactKind,
        project_handle: &str,
        key: &str,
    ) -> Result<bool, String> {
        let artifact_id = artifact_storage_id(kind, &self.account, project_handle, key);
        match self.manifest(&artifact_id)? {
            Some(manifest) => Ok(Path::new(&manifest.blob_path).exists()),
            None => Ok(false),
        }
    }

    pub fn manifest(&self, artifact_id: &str) -> Result<Option<ArtifactManifest>, String> {
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

    pub fn fetch_artifact(
        &self,
        kind: ArtifactKind,
        project_handle: &str,
        key: &str,
    ) -> Result<Option<ArtifactManifest>, String> {
        let artifact_id = artifact_storage_id(kind, &self.account, project_handle, key);
        match self.manifest(&artifact_id)? {
            Some(manifest) if Path::new(&manifest.blob_path).exists() => Ok(Some(manifest)),
            Some(_) => Ok(None),
            None => Ok(None),
        }
    }

    pub fn persist_artifact_from_path(
        &self,
        kind: ArtifactKind,
        project_handle: &str,
        key: &str,
        content_type: &str,
        source_path: &Path,
    ) -> Result<ArtifactManifest, String> {
        let artifact_id = artifact_storage_id(kind, &self.account, project_handle, key);
        let destination = blob_path(&self.data_dir, kind, &artifact_id);
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

    pub fn persist_artifact_from_bytes(
        &self,
        kind: ArtifactKind,
        project_handle: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<ArtifactManifest, String> {
        let temp_path = temp_file_path(&self.tmp_dir.join("uploads"), "replication");
        std::fs::write(&temp_path, bytes)
            .map_err(|error| format!("failed to write temp blob: {error}"))?;
        self.persist_artifact_from_path(kind, project_handle, key, content_type, &temp_path)
    }

    pub fn delete_project(&self, project_handle: &str) -> Result<(), String> {
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

    pub fn start_multipart_upload(
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

    pub fn multipart_upload(&self, upload_id: &str) -> Result<Option<MultipartUpload>, String> {
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

    pub fn add_multipart_part(
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

        let next_total = next_total_size(&upload.parts, part_number, size);
        validate_total_size(next_total, MAX_MODULE_TOTAL_BYTES)?;

        let upload_dir = self.data_dir.join("multipart").join(upload_id);
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

    pub fn complete_multipart_upload(
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

        let assembled_path = temp_file_path(&self.tmp_dir.join("uploads"), "module");
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

    pub fn abort_multipart_upload(&self, upload_id: &str) -> Result<(), String> {
        if let Some(upload) = self.multipart_upload(upload_id)? {
            let _ = std::fs::remove_dir_all(self.data_dir.join("multipart").join(upload_id));
            self.db
                .delete_cf(self.cf(CF_MULTIPART_UPLOADS), upload_id.as_bytes())
                .map_err(|error| format!("failed to delete multipart upload: {error}"))?;

            for part in upload.parts.values() {
                let _ = std::fs::remove_file(&part.path);
            }
        }

        Ok(())
    }

    pub fn enqueue(&self, message: OutboxMessage) -> Result<(), String> {
        let key = format!("{:020}-{}", now_ms(), Uuid::now_v7());
        let value = serde_json::to_vec(&message)
            .map_err(|error| format!("failed to encode outbox message: {error}"))?;
        self.db
            .put_cf(self.cf(CF_OUTBOX), key.as_bytes(), value)
            .map_err(|error| format!("failed to enqueue outbox message: {error}"))
    }

    pub fn outbox_messages(&self) -> Result<Vec<(Vec<u8>, OutboxMessage)>, String> {
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

    pub fn delete_outbox_message(&self, key: &[u8]) -> Result<(), String> {
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

fn next_total_size(parts: &BTreeMap<u32, MultipartPart>, part_number: u32, size: u64) -> u64 {
    let current_total: u64 = parts.values().map(|part| part.size).sum();
    let replaced_size = parts.get(&part_number).map(|part| part.size).unwrap_or(0);
    current_total - replaced_size + size
}

fn validate_total_size(next_total: u64, max_total: u64) -> Result<(), MultipartError> {
    if next_total > max_total {
        Err(MultipartError::TotalSizeExceeded)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    use crate::{config::Config, domain::ReplicationOperation};

    fn temp_store() -> (TempDir, Config, Store) {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let config = Config {
            port: 0,
            account: "test-account".into(),
            region: "local".into(),
            tmp_dir: temp_dir.path().join("tmp"),
            data_dir: temp_dir.path().join("data"),
            node_url: "http://127.0.0.1:0".into(),
            peers: vec!["http://127.0.0.1:0".into()],
            otlp_traces_endpoint: "http://127.0.0.1:4318/v1/traces".into(),
            otel_service_name: "cache-test".into(),
            otel_deployment_environment: "test".into(),
        };
        std::fs::create_dir_all(config.tmp_dir.join("uploads"))
            .expect("failed to create upload temp dir");
        std::fs::create_dir_all(config.data_dir.join("rocksdb"))
            .expect("failed to create rocksdb dir");
        std::fs::create_dir_all(config.data_dir.join("blobs")).expect("failed to create blobs dir");
        std::fs::create_dir_all(config.data_dir.join("multipart"))
            .expect("failed to create multipart dir");
        let store = Store::open(&config).expect("failed to open store");
        (temp_dir, config, store)
    }

    #[test]
    fn persist_and_fetch_artifact_round_trip() {
        let (_temp_dir, _config, store) = temp_store();

        let manifest = store
            .persist_artifact_from_bytes(
                ArtifactKind::Xcode,
                "ios",
                "artifact-1",
                "application/octet-stream",
                b"hello",
            )
            .expect("failed to persist artifact");

        assert!(
            store
                .artifact_exists(ArtifactKind::Xcode, "ios", "artifact-1")
                .expect("failed to check artifact existence")
        );

        let fetched = store
            .fetch_artifact(ArtifactKind::Xcode, "ios", "artifact-1")
            .expect("failed to fetch artifact")
            .expect("artifact should exist");

        assert_eq!(fetched, manifest);
        assert_eq!(
            std::fs::read(&manifest.blob_path).expect("failed to read blob"),
            b"hello"
        );
    }

    #[test]
    fn delete_project_removes_manifests_and_blobs() {
        let (_temp_dir, _config, store) = temp_store();

        let manifest = store
            .persist_artifact_from_bytes(
                ArtifactKind::Gradle,
                "android",
                "gradle-1",
                "application/octet-stream",
                b"gradle-cache",
            )
            .expect("failed to persist artifact");

        store
            .delete_project("android")
            .expect("failed to delete project");

        assert!(
            store
                .fetch_artifact(ArtifactKind::Gradle, "android", "gradle-1")
                .expect("failed to fetch artifact")
                .is_none()
        );
        assert!(!Path::new(&manifest.blob_path).exists());
    }

    #[test]
    fn multipart_upload_round_trip() {
        let (_temp_dir, config, store) = temp_store();
        let upload_id = store
            .start_multipart_upload("acme", "ios", "builds", "hash-1", "Module.framework")
            .expect("failed to start upload");

        let part_1 = config.tmp_dir.join("part-1");
        let part_2 = config.tmp_dir.join("part-2");
        std::fs::write(&part_1, b"part-one-").expect("failed to write part 1");
        std::fs::write(&part_2, b"part-two").expect("failed to write part 2");

        store
            .add_multipart_part(&upload_id, 1, &part_1, 9)
            .expect("failed to store part 1");
        store
            .add_multipart_part(&upload_id, 2, &part_2, 8)
            .expect("failed to store part 2");

        let manifest = store
            .complete_multipart_upload(&upload_id, &[1, 2])
            .expect("failed to complete upload");

        assert_eq!(
            std::fs::read(&manifest.blob_path).expect("failed to read assembled artifact"),
            b"part-one-part-two"
        );
        assert!(
            store
                .multipart_upload(&upload_id)
                .expect("failed to load multipart upload")
                .is_none()
        );
    }

    #[test]
    fn multipart_size_validation_accounts_for_replaced_parts() {
        let mut parts = BTreeMap::new();
        parts.insert(
            1,
            MultipartPart {
                path: "part-1".into(),
                size: 10,
            },
        );
        parts.insert(
            2,
            MultipartPart {
                path: "part-2".into(),
                size: 5,
            },
        );

        assert_eq!(next_total_size(&parts, 1, 8), 13);
        assert_eq!(
            validate_total_size(101, 100),
            Err(MultipartError::TotalSizeExceeded)
        );
        assert_eq!(validate_total_size(100, 100), Ok(()));
    }

    #[test]
    fn outbox_queue_round_trip() {
        let (_temp_dir, _config, store) = temp_store();

        store
            .enqueue(OutboxMessage {
                target: "http://peer".into(),
                operation: ReplicationOperation::DeleteProject {
                    project_handle: "ios".into(),
                },
            })
            .expect("failed to enqueue outbox message");

        let messages = store
            .outbox_messages()
            .expect("failed to read outbox messages");
        assert_eq!(messages.len(), 1);

        let (key, message) = &messages[0];
        assert_eq!(
            *message,
            OutboxMessage {
                target: "http://peer".into(),
                operation: ReplicationOperation::DeleteProject {
                    project_handle: "ios".into(),
                },
            }
        );

        store
            .delete_outbox_message(key)
            .expect("failed to delete outbox message");
        assert!(
            store
                .outbox_messages()
                .expect("failed to read outbox messages")
                .is_empty()
        );
    }
}
