use serde::{Deserialize, Serialize};

use crate::artifact::kind::ArtifactKind;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifest {
    pub artifact_id: String,
    pub kind: ArtifactKind,
    pub namespace_id: String,
    pub key: String,
    pub content_type: String,
    pub blob_path: String,
    pub size: u64,
    pub created_at_ms: u64,
}
