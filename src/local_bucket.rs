use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use crate::bucket::ObjectEntry;

/// A `Bucket` implementation backed by the local filesystem.  The
/// `root` directory acts as the container; object keys are mapped to
/// paths beneath it (parent directories are created as needed).
pub struct LocalBucket {
    root: PathBuf,
}

impl LocalBucket {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalBucket { root: root.into() }
    }

    fn key_path(&self, key: &str) -> PathBuf {
        // Strip a leading '/' so that absolute-looking keys still land
        // safely inside root.
        self.root.join(key.trim_start_matches('/'))
    }

    pub async fn upload_with_progress(
        &self,
        key: &str,
        mut source: File,
        callback: impl Fn(usize) + Sync + Send + 'static,
    ) -> Result<(), String> {
        let dest_path = self.key_path(key);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directories for {key}: {e}"))?;
        }
        let mut dest = File::create(&dest_path)
            .map_err(|e| format!("Failed to create {}: {e}", dest_path.display()))?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = source.read(&mut buf).map_err(|e| format!("Read error: {e}"))?;
            if n == 0 {
                break;
            }
            dest.write_all(&buf[..n]).map_err(|e| format!("Write error: {e}"))?;
            callback(n);
        }
        Ok(())
    }

    pub async fn exists(&self, key: &str) -> Result<bool, String> {
        Ok(self.key_path(key).exists())
    }

    pub async fn download_with_progress(
        &self,
        key: &str,
        mut dest: File,
        callback: impl Fn(usize) + Sync + Send + 'static,
    ) -> io::Result<u64> {
        let mut source = File::open(self.key_path(key))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dest.write_all(&buf[..n])?;
            callback(n);
            total += n as u64;
        }
        Ok(total)
    }

    pub async fn download(&self, key: &str, dest: File) -> io::Result<u64> {
        let mut source = File::open(self.key_path(key))?;
        let mut dest = dest;
        io::copy(&mut source, &mut dest)
    }

    /// Returns up to 100 entries whose key begins with `prefix`, with
    /// keys strictly greater than `marker` (mimicking Swift's pagination).
    pub async fn list(
        &self,
        prefix: Option<&str>,
        marker: Option<&str>,
    ) -> io::Result<Vec<ObjectEntry>> {
        let mut entries: Vec<ObjectEntry> = Vec::new();
        collect_entries(&self.root, &self.root, prefix, &mut entries)?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(m) = marker {
            entries.retain(|e| e.name.as_str() > m);
        }
        entries.truncate(100);
        Ok(entries)
    }
}

fn collect_entries(
    root: &Path,
    dir: &Path,
    prefix: Option<&str>,
    result: &mut Vec<ObjectEntry>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_entries(root, &path, prefix, result)?;
        } else {
            let rel = path.strip_prefix(root).unwrap();
            let name = rel.to_string_lossy().replace('\\', "/");
            if let Some(p) = prefix {
                if !name.starts_with(p) {
                    continue;
                }
            }
            let meta = fs::metadata(&path)?;
            let bytes = meta.len() as i128;
            let last_modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| {
                    chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.6f").to_string())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            result.push(ObjectEntry {
                hash: String::new(),
                last_modified,
                bytes,
                name,
                content_type: String::from("application/octet-stream"),
            });
        }
    }
    Ok(())
}
