use serde::{Deserialize, Serialize};

use crate::artifact::kind::ArtifactKind;

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

#[cfg(test)]
mod tests {
    use crate::artifact::kind::ArtifactKind;

    use super::ReplicationOperation;

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
