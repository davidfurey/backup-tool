use std::sync::Arc;
use std::os::unix::prelude::PermissionsExt;
use std::time::Duration;
use std::path::{Component, PathBuf, Path};

use futures::{StreamExt, FutureExt};
use log::{trace, error, info};
use sha2::{Sha256, Digest};
use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use crate::{datastore, hash};
use datastore::DataStore;
use crate::metadata_file::FileMetadata;
use crate::decryption;
use std::fs::{File, set_permissions, create_dir_all, remove_dir_all};
use std::os::unix::fs::symlink;
use crate::filetype;
use filetype::FileType;
use crate::swift::Bucket;
use crate::utils::humanise_bytes;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use filetime::{set_file_mtime, set_symlink_file_times, FileTime};
use rand::{distributions::Alphanumeric, Rng};
use fs2::free_space;

/// Converts an entry name from the metadata database into a safe,
/// normalised relative [`PathBuf`] that is guaranteed to contain no
/// `..` components and no leading root separator.
///
/// Any `..` (ParentDir), absolute root, or prefix component causes the
/// function to return `None`; the entry will be skipped.
///
/// Returns `None` if the resulting path is empty or otherwise unsafe.
fn safe_relative_path(name: &str) -> Option<PathBuf> {
    let mut result = PathBuf::new();

    for component in Path::new(name).components() {
        match component {
            Component::Normal(c) => result.push(c),
            Component::CurDir => {
                // '.' is a no-op; skip it.
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                error!(
                    "Rejecting entry {:?}: unsafe path component (must be a plain relative path)",
                    name
                );
                return None;
            }
        }
    }

    if result.as_os_str().is_empty() {
        error!("Rejecting entry {:?}: empty path after normalisation", name);
        return None;
    }

    Some(result)
}

/// SHA-256 of the file at `path`, returned as a lowercase hex string.
fn sha256_file(path: &PathBuf) -> String {
  let mut file = File::open(path).unwrap();
  let mut hasher = Sha256::new();
  std::io::copy(&mut file, &mut hasher).unwrap();
  format!("{:x}", hasher.finalize())
}

async fn download_file(data_hash: &str, destination: PathBuf, bucket: &Bucket, data_prefix: &str, cert: &Cert, cache: &PathBuf, hmac_secret: &String, mp: &MultiProgress) {
  // todo: avoid repeating downloads

  // Use a short random suffix so that concurrent tasks downloading the same
  // hash don't collide on the temp filenames.
  let random_suffix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(4)
    .map(char::from)
    .collect();
  let encrypted_temp = cache.join(format!("{}{}.gpg", data_hash, random_suffix));
  let decrypted_temp = cache.join(format!("{}{}.plain", data_hash, random_suffix));

  let pb = mp.add(ProgressBar::new_spinner());
  pb.set_style(
    ProgressStyle::with_template("{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {msg} ({bytes} downloaded)")
      .unwrap()
      .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
  );
  pb.set_prefix("[Download]");
  pb.set_message(data_hash[..16].to_string());
  pb.enable_steady_tick(Duration::from_millis(80));

  let encrypted_file = File::create(&encrypted_temp).unwrap();
  let key = format!("{}{}", data_prefix, data_hash);
  let pb_cb = pb.clone();
  bucket.download_with_progress(key.as_str(), encrypted_file, move |bytes| {
    pb_cb.inc(bytes as u64);
  }).await.unwrap();
  pb.finish_and_clear();
  trace!("downloaded {:?}", encrypted_temp);

  // Decrypt into a temp file so the final path only appears once the hash
  // check has passed.
  {
    let mut source = File::open(&encrypted_temp).unwrap();
    let mut dest = File::create(&decrypted_temp).unwrap();
    decryption::decrypt_file(&mut source, &mut dest, cert, None).unwrap();
  }
  std::fs::remove_file(&encrypted_temp).unwrap();

  if hash::data(&decrypted_temp, hmac_secret) != data_hash {
    std::fs::remove_file(&decrypted_temp).unwrap();
    panic!("Data hash did not match for {:?}", destination);
  }

  std::fs::rename(&decrypted_temp, &destination).unwrap();
  trace!("restored {:?}", destination);
}

pub async fn process_file(entry: &FileMetadata, destination: PathBuf, data_bucket: &Bucket, data_prefix: &str, data_cache: &PathBuf, key: &Cert, hmac_secret: &String, mp: &MultiProgress) -> i64 {
  // Normalise the stored name to a safe relative path before joining.
  // safe_relative_path rejects any '..' component and strips a legacy
  // leading '/' so that joining onto `destination` is always safe.
  let rel = match safe_relative_path(entry.name.as_str()) {
    Some(p) => p,
    None => return 0,
  };
  let path = destination.join(&rel);
  match entry.ttype {
      FileType::FILE => {
        trace!("Creating file {:?}", &path);
        let data_hash = match &entry.data_hash {
            Some(val) => val.as_str(), 
            None => "",
        };
        let downloaded = download_file(
          data_hash, 
          path.clone(), 
          &data_bucket, 
          data_prefix, 
          &key, 
          &data_cache,
          hmac_secret,
          mp,
        );
        let mtime = FileTime::from_unix_time(entry.mtime, 0);
        downloaded.map(move |f| {
          set_file_mtime(&path, mtime).unwrap();
          let permissions = PermissionsExt::from_mode(entry.mode);
          set_permissions(&path, permissions).unwrap();
          f
        }).await;
        1
      }
      FileType::SYMLINK => {
        trace!("Creating symlink {:?} -> {:?}", &path, entry.destination);
        symlink(entry.destination.clone().unwrap(), &path).unwrap();
        // Symlink permissions are not meaningful on Linux (always rwxrwxrwx)
        // and cannot be set via std::fs::set_permissions.
        let mtime = FileTime::from_unix_time(entry.mtime, 0);
        set_symlink_file_times(&path, mtime, mtime).unwrap();
        1
      }
      FileType::DIRECTORY => {
        trace!("Creating dir {:?}", &path);
        create_dir_all(path.as_path()).unwrap();
        let permissions = PermissionsExt::from_mode(entry.mode);
        set_permissions(&path, permissions).unwrap();
        // Note: mtime is intentionally NOT set here — it will be overwritten
        // as files are written into the directory. A second pass in
        // restore_backup applies directory mtimes after all content is written.
        1
      }
    }
}

pub async fn validate_backup(backup: &str, stores: &[DataStore], key_file: PathBuf, signing_key_file: &Option<PathBuf>, mp: MultiProgress) -> bool {
  assert!(!stores.is_empty(), "At least one store is required");

  // Temp dir for all metadata work — cleaned up at the end.
  let tmp_suffix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(8)
    .map(char::from)
    .collect();
  let tmp_dir = std::env::temp_dir().join(format!("backup-validate-{}", tmp_suffix));
  create_dir_all(&tmp_dir).unwrap();

  let key = Cert::from_file(key_file).unwrap();

  // Download and decrypt the metadata file from every store, then hash the
  // decrypted content so the comparison isn't fooled by ciphertext nondeterminism.
  info!("Downloading metadata from {} store(s)...", stores.len());
  let mut store_meta: Vec<(i32, PathBuf, String)> = Vec::new(); // (store_id, decrypted_path, sha256)
  for store in stores.iter() {
    let encrypted_path = tmp_dir.join(format!("metadata-store-{}.encrypted", store.id));
    let decrypted_path = tmp_dir.join(format!("metadata-store-{}.sqlite", store.id));
    {
      let encrypted_file = File::create(&encrypted_path).unwrap();
      let bucket = store.init().await;
      let prefix = &store.metadata_prefix;
      bucket.download(format!("{prefix}{backup}.metadata").as_str(), encrypted_file).await.unwrap();
    }
    {
      let mut source = File::open(&encrypted_path).unwrap();
      let mut dest = File::create(&decrypted_path).unwrap();
      let signing_key = signing_key_file.clone().map(|x| Cert::from_file(x).unwrap());
      decryption::decrypt_file(&mut source, &mut dest, &key, signing_key).unwrap();
    }
    std::fs::remove_file(&encrypted_path).unwrap();
    let hash = sha256_file(&decrypted_path);
    info!("  store={}  metadata sha256={:.16}", store.id, &hash);
    store_meta.push((store.id, decrypted_path, hash));
  }

  // Verify all stores carry identical metadata.
  let reference_hash = &store_meta[0].2;
  let mut metadata_ok = true;
  for (store_id, _, hash) in &store_meta {
    if hash != reference_hash {
      error!("METADATA MISMATCH  store={}  sha256={:.16}  (reference from store {} is {:.16})",
             store_id, hash, stores[0].id, reference_hash);
      metadata_ok = false;
    }
  }
  if !metadata_ok {
    error!("Metadata files differ across stores — aborting validation");
    remove_dir_all(&tmp_dir).unwrap();
    return false;
  }
  if stores.len() > 1 {
    info!("Metadata is identical across all {} stores", stores.len());
  }

  // Use the first store's decrypted copy for content validation; discard the rest.
  let (_, first_decrypted, _) = store_meta.remove(0);
  for (_, decrypted_path, _) in &store_meta {
    std::fs::remove_file(decrypted_path).unwrap();
  }
  let metadata_file = first_decrypted;

  let metadata_reader = crate::metadata_file::MetadataReader::new(metadata_file).await;

  let checker_pb = mp.add(ProgressBar::new_spinner());
  checker_pb.set_style(
    ProgressStyle::with_template("{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {pos} files checked{msg}")
      .unwrap()
      .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
  );
  checker_pb.set_prefix("[Validate]");
  checker_pb.enable_steady_tick(Duration::from_millis(80));

  // Initialise one data bucket per store up-front to avoid re-authenticating
  // for every file. Wrapped in Arc so the shared reference can be moved into
  // each per-file future without cloning the Buckets themselves.
  let buckets: Arc<Vec<(i32, String, Bucket)>> = Arc::new(
    futures::stream::iter(stores)
      .then(|s| async move { (s.id, s.data_prefix.clone(), s.init().await) })
      .collect()
      .await
  );

  // Stream metadata entries directly; check each FILE's hash against every
  // store concurrently. Folding into (total, missing, errors) avoids
  // collecting the full file list into memory. `missing` counts definitive
  // 404 responses; `errors` counts unexpected statuses / request failures
  // that may indicate auth or Swift outages rather than absent data.
  let (total_files, missing, errors): (u64, u64, u64) = metadata_reader.read().await
    .filter(|e| futures::future::ready(matches!(&e.ttype, FileType::FILE) && e.data_hash.is_some()))
    .map(|e| {
      let buckets = Arc::clone(&buckets);
      async move {
        let data_hash = e.data_hash.unwrap();
        let mut file_missing: u64 = 0;
        let mut file_errors: u64 = 0;
        for (store_id, data_prefix, bucket) in buckets.iter() {
          let key = format!("{}{}", data_prefix, data_hash);
          match bucket.exists(&key).await {
            Ok(true) => {
              trace!("OK  store={}  hash={}", store_id, &data_hash[..16]);
            }
            Ok(false) => {
              error!("MISSING  store={}  hash={}  file={}", store_id, &data_hash[..16], e.name);
              file_missing += 1;
            }
            Err(_) => {
              // exists() has already logged the status/error detail.
              error!("ERROR  store={}  hash={}  file={} (could not verify — see above)", store_id, &data_hash[..16], e.name);
              file_errors += 1;
            }
          }
        }
        (1u64, file_missing, file_errors)
      }
    })
    .buffer_unordered(16)
    .fold((0u64, 0u64, 0u64), |acc, (t, m, e)| {
      checker_pb.inc(1);
      futures::future::ready((acc.0 + t, acc.1 + m, acc.2 + e))
    })
    .await;

  remove_dir_all(&tmp_dir).unwrap();

  if missing == 0 && errors == 0 {
    checker_pb.finish_with_message(format!(" — passed ({} store(s))", stores.len()));
    true
  } else {
    let total_checks = total_files * stores.len() as u64;
    let mut parts: Vec<String> = Vec::new();
    if missing > 0 {
      parts.push(format!("{}/{} missing", missing, total_checks));
    }
    if errors > 0 {
      parts.push(format!("{} check error(s) (auth/network/Swift failure)", errors));
    }
    checker_pb.finish_with_message(format!(" — FAILED: {}", parts.join(", ")));
    false
  }
}

pub async fn restore_backup(destination: PathBuf, backup: &String, metadata_store: &DataStore, data_store: &DataStore, key_file: PathBuf, hmac_secret: &String, signing_key_file: &Option<PathBuf>, mp: MultiProgress) {

  if destination.exists() {
    error!("Bailing because destination already exists");
    return;
  }

  // Use a random suffix for the temp dir so it cannot collide with a
  // top-level ".data" path that happens to be present in the backup itself.
  let tmp_suffix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(8)
    .map(char::from)
    .collect();
  let temporary_data_dir = destination.join(format!(".backup-tmp-{}", tmp_suffix));
  create_dir_all(&temporary_data_dir).unwrap();

  let key = &Cert::from_file(key_file).unwrap();
  
  let metadata_file = temporary_data_dir.join("metadata.sqlite");

  {
    let encrypted_metadata_file = temporary_data_dir.join("metadata");
    
    trace!("creating {:?}", encrypted_metadata_file);
    
    {
      let encrypted_file = File::create(&encrypted_metadata_file).unwrap();
      let bucket = metadata_store.init().await;
      let prefix = &metadata_store.metadata_prefix;
      bucket.download(format!("{prefix}{backup}.metadata").as_str(), encrypted_file).await.unwrap();
    }
    
    {
      let mut source = File::open(&encrypted_metadata_file).unwrap();
      let mut dest = File::create(&metadata_file).unwrap();
      let signing_key = signing_key_file.clone().map(|x| Cert::from_file(x).unwrap());
      decryption::decrypt_file(&mut source, &mut dest, &key, signing_key).unwrap();
    }
  }

  let metadata_reader = crate::metadata_file::MetadataReader::new(metadata_file.clone()).await;

  let size: u64 = metadata_reader.read_metadata("size").await.parse().unwrap();
  info!("Backup is {}", humanise_bytes(size));

  let available_space = free_space(destination.as_path()).unwrap();
  if available_space < size {
    panic!("Backup is {} but disk only has {} available space", humanise_bytes(size), humanise_bytes(available_space));
  }

  let counter_pb = mp.add(ProgressBar::new_spinner());
  counter_pb.set_style(
    ProgressStyle::with_template("{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {pos} files restored")
      .unwrap()
      .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
  );
  counter_pb.set_prefix("[Restore]");
  counter_pb.enable_steady_tick(Duration::from_millis(80));

  let data_cache = &temporary_data_dir;
  trace!("Destination: {:?}", destination.as_path());
  let destination = &destination;
  let data_bucket = &data_store.init().await;
  let mp_ref = &mp;
  let counter_inc = counter_pb.clone();
  metadata_reader.read().await
    .map(|entry| async move {
      process_file(&entry, destination.clone(), &data_bucket, &data_store.data_prefix, &data_cache, &key, hmac_secret, mp_ref).await
    })
    .buffer_unordered(4)
    .for_each(|_| { counter_inc.inc(1); futures::future::ready(()) })
    .await;

  counter_pb.finish_with_message("done");

  // Second pass: apply directory mtimes.
  // Directory mtimes are updated whenever files or subdirectories are created
  // inside them, so they must be set after all content has been restored.
  // Process deepest paths first (reverse lexicographic order) so that setting
  // a child directory's mtime does not cause its parent to be re-stamped.
  {
    let dir_metadata_reader = crate::metadata_file::MetadataReader::new(metadata_file).await;
    let mut dir_entries: Vec<FileMetadata> = dir_metadata_reader.read().await
      .filter(|entry| futures::future::ready(matches!(entry.ttype, FileType::DIRECTORY)))
      .collect()
      .await;
    dir_entries.sort_by(|a, b| b.name.cmp(&a.name));
    for entry in dir_entries {
      let rel = match safe_relative_path(entry.name.as_str()) {
        Some(p) => p,
        None => continue,
      };
      let path = destination.join(&rel);
      let mtime = FileTime::from_unix_time(entry.mtime, 0);
      set_file_mtime(&path, mtime).unwrap();
    }
  }

  remove_dir_all(&temporary_data_dir).unwrap();
}