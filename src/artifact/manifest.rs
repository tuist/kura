use serde::{Deserialize, Serialize};

use crate::artifact::{
    class::ArtifactClass, client::ArtifactClient, kind::ArtifactKind, metadata::ArtifactMetadata,
    storage_kind::StorageKind,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifest {
    pub artifact_id: String,
    pub kind: ArtifactKind,
    #[serde(default)]
    pub client: ArtifactClient,
    #[serde(default)]
    pub artifact_class: ArtifactClass,
    pub namespace_id: String,
    pub key: String,
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_offset: Option<u64>,
    pub size: u64,
    #[serde(default)]
    pub version_ms: u64,
    #[serde(default)]
    pub created_at_ms: u64,
}

impl ArtifactManifest {
    pub fn is_segment_backed(&self) -> bool {
        self.segment_id.is_some()
    }

    pub fn logical_key(&self) -> &str {
        &self.key
    }

    pub fn storage_kind(&self) -> StorageKind {
        if self.segment_id.is_some() {
            StorageKind::Segment
        } else if self.blob_path.is_some() {
            StorageKind::FilesystemBlob
        } else {
            StorageKind::RocksdbInline
        }
    }

    pub fn metadata(&self, tenant_id: &str) -> ArtifactMetadata {
        ArtifactMetadata {
            tenant_id: tenant_id.to_owned(),
            namespace_id: self.namespace_id.clone(),
            client: self.client,
            artifact_class: self.artifact_class,
            logical_key: self.logical_key().to_owned(),
            storage_kind: self.storage_kind(),
            content_type: self.content_type.clone(),
            size_bytes: self.size,
            version_ms: self.version_ms,
            created_at_ms: self.created_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::artifact::{
        class::ArtifactClass, client::ArtifactClient, kind::ArtifactKind, storage_kind::StorageKind,
    };

    use super::ArtifactManifest;

    #[test]
    fn exposes_normalized_storage_metadata() {
        let manifest = ArtifactManifest {
            artifact_id: "artifact".into(),
            kind: ArtifactKind::Keyvalue,
            client: ArtifactClient::Generic,
            artifact_class: ArtifactClass::ActionCache,
            namespace_id: "ios".into(),
            key: "action-key".into(),
            content_type: "application/json".into(),
            blob_path: None,
            segment_id: None,
            segment_offset: None,
            size: 128,
            version_ms: 100,
            created_at_ms: 90,
        };

        let metadata = manifest.metadata("acme");
        assert_eq!(metadata.tenant_id, "acme");
        assert_eq!(metadata.namespace_id, "ios");
        assert_eq!(metadata.client, ArtifactClient::Generic);
        assert_eq!(metadata.artifact_class, ArtifactClass::ActionCache);
        assert_eq!(metadata.logical_key, "action-key");
        assert_eq!(metadata.storage_kind, StorageKind::RocksdbInline);
        assert_eq!(metadata.content_type, "application/json");
        assert_eq!(metadata.size_bytes, 128);
        assert_eq!(metadata.version_ms, 100);
        assert_eq!(metadata.created_at_ms, 90);
    }
}
