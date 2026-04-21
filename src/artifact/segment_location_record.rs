use crate::artifact::{
    class::ArtifactClass, client::ArtifactClient, kind::ArtifactKind, manifest::ArtifactManifest,
};

const SEGMENT_LOCATION_RECORD_VERSION_V1: u8 = 1;
const SEGMENT_LOCATION_RECORD_VERSION_V2: u8 = 2;
const SEGMENT_LOCATION_RECORD_VERSION_V3: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentLocationRecord {
    pub kind: ArtifactKind,
    pub client: ArtifactClient,
    pub artifact_class: ArtifactClass,
    pub namespace_id: String,
    pub key: String,
    pub content_type: String,
    pub segment_id: String,
    pub segment_offset: u64,
    pub size: u64,
    pub version_ms: u64,
    pub created_at_ms: u64,
}

impl SegmentLocationRecord {
    pub fn from_manifest(manifest: &ArtifactManifest) -> Result<Self, String> {
        let segment_id = manifest
            .segment_id
            .clone()
            .ok_or_else(|| "segment-backed manifest is missing segment id".to_string())?;
        let segment_offset = manifest
            .segment_offset
            .ok_or_else(|| "segment-backed manifest is missing segment offset".to_string())?;

        Ok(Self {
            kind: manifest.kind,
            client: manifest.client,
            artifact_class: manifest.artifact_class,
            namespace_id: manifest.namespace_id.clone(),
            key: manifest.key.clone(),
            content_type: manifest.content_type.clone(),
            segment_id,
            segment_offset,
            size: manifest.size,
            version_ms: manifest.version_ms,
            created_at_ms: manifest.created_at_ms,
        })
    }

    pub fn into_manifest(self, artifact_id: &str) -> ArtifactManifest {
        ArtifactManifest {
            artifact_id: artifact_id.to_owned(),
            kind: self.kind,
            client: self.client,
            artifact_class: self.artifact_class,
            namespace_id: self.namespace_id,
            key: self.key,
            content_type: self.content_type,
            blob_path: None,
            segment_id: Some(self.segment_id),
            segment_offset: Some(self.segment_offset),
            size: self.size,
            version_ms: self.version_ms,
            created_at_ms: self.created_at_ms,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(
            2 + 8
                + 8
                + 8
                + self.namespace_id.len()
                + self.key.len()
                + self.content_type.len()
                + self.segment_id.len()
                + 16,
        );
        bytes.push(SEGMENT_LOCATION_RECORD_VERSION_V3);
        bytes.push(kind_code(self.kind));
        bytes.push(client_code(self.client));
        bytes.push(class_code(self.artifact_class));
        bytes.extend_from_slice(&self.segment_offset.to_le_bytes());
        bytes.extend_from_slice(&self.size.to_le_bytes());
        bytes.extend_from_slice(&self.version_ms.to_le_bytes());
        bytes.extend_from_slice(&self.created_at_ms.to_le_bytes());
        push_string(&mut bytes, &self.namespace_id);
        push_string(&mut bytes, &self.key);
        push_string(&mut bytes, &self.content_type);
        push_string(&mut bytes, &self.segment_id);
        bytes
    }

    pub fn decode(bytes: &[u8], artifact_id: &str) -> Result<Option<ArtifactManifest>, String> {
        let Some(version) = bytes.first().copied() else {
            return Ok(None);
        };
        if version != SEGMENT_LOCATION_RECORD_VERSION_V1
            && version != SEGMENT_LOCATION_RECORD_VERSION_V2
            && version != SEGMENT_LOCATION_RECORD_VERSION_V3
        {
            return Ok(None);
        }
        let mut cursor = 1;
        let kind = decode_kind(read_u8(bytes, &mut cursor)?)?;
        let client = if version == SEGMENT_LOCATION_RECORD_VERSION_V3 {
            decode_client(read_u8(bytes, &mut cursor)?)?
        } else {
            kind.client()
        };
        let artifact_class = if version == SEGMENT_LOCATION_RECORD_VERSION_V3 {
            decode_class(read_u8(bytes, &mut cursor)?)?
        } else {
            kind.artifact_class()
        };
        let segment_offset = read_u64(bytes, &mut cursor)?;
        let size = read_u64(bytes, &mut cursor)?;
        let version_ms = if version == SEGMENT_LOCATION_RECORD_VERSION_V2 {
            read_u64(bytes, &mut cursor)?
        } else if version == SEGMENT_LOCATION_RECORD_VERSION_V3 {
            read_u64(bytes, &mut cursor)?
        } else {
            0
        };
        let created_at_ms = read_u64(bytes, &mut cursor)?;
        let namespace_id = read_string(bytes, &mut cursor)?;
        let key = read_string(bytes, &mut cursor)?;
        let content_type = read_string(bytes, &mut cursor)?;
        let segment_id = read_string(bytes, &mut cursor)?;

        Ok(Some(
            Self {
                kind,
                client,
                artifact_class,
                namespace_id,
                key,
                content_type,
                segment_id,
                segment_offset,
                size,
                version_ms: if version_ms == 0 {
                    created_at_ms
                } else {
                    version_ms
                },
                created_at_ms,
            }
            .into_manifest(artifact_id),
        ))
    }
}

fn kind_code(kind: ArtifactKind) -> u8 {
    match kind {
        ArtifactKind::Keyvalue => 0,
        ArtifactKind::Xcode => 1,
        ArtifactKind::Gradle => 2,
        ArtifactKind::Module => 3,
    }
}

fn client_code(client: ArtifactClient) -> u8 {
    match client {
        ArtifactClient::Generic => 0,
        ArtifactClient::Xcode => 1,
        ArtifactClient::Gradle => 2,
        ArtifactClient::Module => 3,
    }
}

fn class_code(class: ArtifactClass) -> u8 {
    match class {
        ArtifactClass::Blob => 0,
        ArtifactClass::ActionCache => 1,
    }
}

fn decode_kind(code: u8) -> Result<ArtifactKind, String> {
    match code {
        0 => Ok(ArtifactKind::Keyvalue),
        1 => Ok(ArtifactKind::Xcode),
        2 => Ok(ArtifactKind::Gradle),
        3 => Ok(ArtifactKind::Module),
        _ => Err(format!("invalid artifact kind code {code}")),
    }
}

fn decode_client(code: u8) -> Result<ArtifactClient, String> {
    match code {
        0 => Ok(ArtifactClient::Generic),
        1 => Ok(ArtifactClient::Xcode),
        2 => Ok(ArtifactClient::Gradle),
        3 => Ok(ArtifactClient::Module),
        _ => Err(format!("invalid artifact client code {code}")),
    }
}

fn decode_class(code: u8) -> Result<ArtifactClass, String> {
    match code {
        0 => Ok(ArtifactClass::Blob),
        1 => Ok(ArtifactClass::ActionCache),
        _ => Err(format!("invalid artifact class code {code}")),
    }
}

fn push_string(bytes: &mut Vec<u8>, value: &str) {
    let len = value.len() as u32;
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, String> {
    let value = *bytes
        .get(*cursor)
        .ok_or_else(|| "segment location record ended unexpectedly".to_string())?;
    *cursor += 1;
    Ok(value)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let next = cursor.saturating_add(8);
    let slice = bytes
        .get(*cursor..next)
        .ok_or_else(|| "segment location record ended unexpectedly".to_string())?;
    *cursor = next;
    Ok(u64::from_le_bytes(
        slice
            .try_into()
            .expect("8-byte slice should convert into u64 bytes"),
    ))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let next = cursor.saturating_add(4);
    let slice = bytes
        .get(*cursor..next)
        .ok_or_else(|| "segment location record ended unexpectedly".to_string())?;
    *cursor = next;
    Ok(u32::from_le_bytes(
        slice
            .try_into()
            .expect("4-byte slice should convert into u32 bytes"),
    ))
}

fn read_string(bytes: &[u8], cursor: &mut usize) -> Result<String, String> {
    let len = read_u32(bytes, cursor)? as usize;
    let next = cursor.saturating_add(len);
    let slice = bytes
        .get(*cursor..next)
        .ok_or_else(|| "segment location record ended unexpectedly".to_string())?;
    *cursor = next;
    String::from_utf8(slice.to_vec())
        .map_err(|error| format!("segment location record contains invalid utf-8: {error}"))
}

#[cfg(test)]
mod tests {
    use super::SegmentLocationRecord;
    use crate::artifact::{
        class::ArtifactClass, client::ArtifactClient, kind::ArtifactKind,
        manifest::ArtifactManifest,
    };

    #[test]
    fn round_trips_segment_backed_manifest() {
        let manifest = ArtifactManifest {
            artifact_id: "artifact".into(),
            kind: ArtifactKind::Gradle,
            client: ArtifactClient::Gradle,
            artifact_class: ArtifactClass::Blob,
            namespace_id: "android".into(),
            key: "cache-key".into(),
            content_type: "application/octet-stream".into(),
            blob_path: None,
            segment_id: Some("segment-1".into()),
            segment_offset: Some(42),
            size: 512,
            version_ms: 5678,
            created_at_ms: 1234,
        };

        let record = SegmentLocationRecord::from_manifest(&manifest)
            .expect("segment-backed manifest should encode");
        let decoded = SegmentLocationRecord::decode(&record.encode(), &manifest.artifact_id)
            .expect("record should decode")
            .expect("record should be present");

        assert_eq!(decoded, manifest);
    }

    #[test]
    fn ignores_non_record_payloads() {
        assert!(
            SegmentLocationRecord::decode(br#"{"artifact_id":"legacy"}"#, "artifact")
                .expect("legacy payload should not error")
                .is_none()
        );
    }

    #[test]
    fn decodes_v1_records_by_promoting_created_at_to_version() {
        let mut bytes = Vec::new();
        bytes.push(1);
        bytes.push(2);
        bytes.extend_from_slice(&42_u64.to_le_bytes());
        bytes.extend_from_slice(&512_u64.to_le_bytes());
        bytes.extend_from_slice(&1234_u64.to_le_bytes());
        for value in [
            "android",
            "cache-key",
            "application/octet-stream",
            "segment-1",
        ] {
            bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }

        let decoded = SegmentLocationRecord::decode(&bytes, "artifact")
            .expect("v1 record should decode")
            .expect("v1 record should produce a manifest");
        assert_eq!(decoded.kind, ArtifactKind::Gradle);
        assert_eq!(decoded.segment_id.as_deref(), Some("segment-1"));
        assert_eq!(decoded.segment_offset, Some(42));
        assert_eq!(decoded.version_ms, 1234);
        assert_eq!(decoded.created_at_ms, 1234);
        assert_eq!(decoded.client, ArtifactClient::Gradle);
        assert_eq!(decoded.artifact_class, ArtifactClass::Blob);
    }
}
