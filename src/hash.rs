// use datastore::DataStore;

use std::io;
use std::fs;
use std::path::Path;
use sha2::{Sha256, Digest};
use hmac::{Hmac, Mac};
// use sharding::ShardedChannel;
use std::os::unix::ffi::OsStrExt;
// use swift::Bucket;
// use sqlite_cache::Cache;

pub fn metadata(len: u64, mtime: i64, path: &Path) -> String {
  let mut hasher = Sha256::new();
  hasher.update(len.to_ne_bytes());
  hasher.update(mtime.to_ne_bytes());
  hasher.update(path.as_os_str().as_bytes());
  format!("{:X}", hasher.finalize())
}

pub fn data(path: &Path, hmac_secret: &str) -> String {
  type HmacSha256 = Hmac<Sha256>;
  let mut hasher = HmacSha256::new_from_slice(hmac_secret.as_bytes())
      .expect("HMAC can take key of any size");
  let mut file = fs::File::open(path).unwrap();
  io::copy(&mut file, &mut hasher).unwrap();
  let digest = hasher.finalize().into_bytes();
  return format!("{:X}", digest);
}