pub const MAX_XCODE_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_GRADLE_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_MODULE_PART_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_MODULE_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const REPLICATION_RETRY_SECS: u64 = 2;

pub const CF_MANIFESTS: &str = "manifests";
pub const CF_PROJECT_ARTIFACTS: &str = "project_artifacts";
pub const CF_MULTIPART_UPLOADS: &str = "multipart_uploads";
pub const CF_OUTBOX: &str = "outbox";
