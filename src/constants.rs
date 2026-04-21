pub const MAX_XCODE_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_GRADLE_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_MODULE_PART_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_MODULE_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const REPLICATION_RETRY_SECS: u64 = 2;

pub const ROCKSDB_CF_MANIFESTS: &str = "manifests";
// Keep the on-disk column family name stable to avoid migrating existing data.
pub const ROCKSDB_CF_NAMESPACE_ARTIFACTS: &str = "project_artifacts";
pub const ROCKSDB_CF_MULTIPART_UPLOADS: &str = "multipart_uploads";
pub const ROCKSDB_CF_OUTBOX: &str = "outbox";
