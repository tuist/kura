use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifest {
    pub artifact_id: String,
    pub kind: ArtifactKind,
    pub project_handle: String,
    pub key: String,
    pub content_type: String,
    pub blob_path: String,
    pub size: u64,
    pub created_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Keyvalue,
    Xcode,
    Gradle,
    Module,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyvalue => "keyvalue",
            Self::Xcode => "xcode",
            Self::Gradle => "gradle",
            Self::Module => "module",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "keyvalue" => Some(Self::Keyvalue),
            "xcode" => Some(Self::Xcode),
            "gradle" => Some(Self::Gradle),
            "module" => Some(Self::Module),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultipartUpload {
    pub upload_id: String,
    pub account_handle: String,
    pub project_handle: String,
    pub category: String,
    pub hash: String,
    pub name: String,
    pub parts: BTreeMap<u32, MultipartPart>,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultipartPart {
    pub path: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboxMessage {
    pub target: String,
    pub operation: ReplicationOperation,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplicationOperation {
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

impl ReplicationOperation {
    pub fn name(&self) -> &'static str {
        match self {
            Self::UpsertArtifact { .. } => "upsert_artifact",
            Self::DeleteProject { .. } => "delete_project",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CompleteMultipartRequest {
    pub parts: Vec<u32>,
}

#[derive(Debug, Deserialize)]
pub struct KeyValuePutRequest {
    pub cas_id: String,
    pub entries: Vec<KeyValueEntry>,
}

#[derive(Debug, Deserialize)]
pub struct KeyValueEntry {
    pub value: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MultipartError {
    NotFound,
    TotalSizeExceeded,
    PartsMismatch,
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_kind_roundtrips() {
        for kind in [
            ArtifactKind::Keyvalue,
            ArtifactKind::Xcode,
            ArtifactKind::Gradle,
            ArtifactKind::Module,
        ] {
            assert_eq!(ArtifactKind::from_str(kind.as_str()), Some(kind));
        }
        assert_eq!(ArtifactKind::from_str("unknown"), None);
    }

    #[test]
    fn replication_operation_names_match_routes() {
        assert_eq!(
            ReplicationOperation::UpsertArtifact {
                kind: ArtifactKind::Xcode,
                project_handle: "ios".into(),
                key: "artifact".into(),
                content_type: "application/octet-stream".into(),
                artifact_id: "artifact-id".into(),
            }
            .name(),
            "upsert_artifact"
        );
        assert_eq!(
            ReplicationOperation::DeleteProject {
                project_handle: "ios".into()
            }
            .name(),
            "delete_project"
        );
    }
}
