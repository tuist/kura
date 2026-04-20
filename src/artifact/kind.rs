use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::ArtifactKind;

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
}
