use std::fs::File;
use crate::swift::SwiftBucket;
use crate::local_bucket::LocalBucket;

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct ObjectEntry {
    pub hash: String,
    pub last_modified: String,
    pub bytes: i128,
    pub name: String,
    pub content_type: String,
}

/// A storage bucket — either a Swift container or a local directory.
/// All callers work with this type; the underlying implementation is
/// selected when the [`DataStore`](crate::datastore::DataStore) is initialised.
pub enum Bucket {
    Swift(SwiftBucket),
    Local(LocalBucket),
}

impl Bucket {
    pub async fn upload_with_progress(
        &self,
        key: &str,
        source: File,
        callback: impl Fn(usize) + Sync + Send + 'static,
    ) -> Result<(), String> {
        match self {
            Bucket::Swift(b) => b.upload_with_progress(key, source, callback).await,
            Bucket::Local(b) => b.upload_with_progress(key, source, callback).await,
        }
    }

    pub async fn exists(&self, key: &str) -> Result<bool, String> {
        match self {
            Bucket::Swift(b) => b.exists(key).await,
            Bucket::Local(b) => b.exists(key).await,
        }
    }

    pub async fn download_with_progress(
        &self,
        key: &str,
        dest: File,
        callback: impl Fn(usize) + Sync + Send + 'static,
    ) -> std::io::Result<u64> {
        match self {
            Bucket::Swift(b) => b.download_with_progress(key, dest, callback).await,
            Bucket::Local(b) => b.download_with_progress(key, dest, callback).await,
        }
    }

    pub async fn download(&self, key: &str, dest: File) -> std::io::Result<u64> {
        match self {
            Bucket::Swift(b) => b.download(key, dest).await,
            Bucket::Local(b) => b.download(key, dest).await,
        }
    }

    pub async fn list(
        &self,
        prefix: Option<&str>,
        marker: Option<&str>,
    ) -> std::io::Result<Vec<ObjectEntry>> {
        match self {
            Bucket::Swift(b) => b.list(prefix, marker).await,
            Bucket::Local(b) => b.list(prefix, marker).await,
        }
    }
}
