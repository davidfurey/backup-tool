use std::os::unix::prelude::PermissionsExt;
use std::path::{PathBuf, Path};

use futures::{StreamExt, FutureExt};
use log::{trace, error, info};
use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use crate::{datastore, hash};
use datastore::DataStore;
extern crate rmp_serde as rmps;
use crate::metadata_file::FileMetadata;
use crate::decryption;
use std::fs::{File, set_permissions, create_dir_all, remove_dir_all};
use std::os::unix::fs::symlink;
use crate::filetype;
use filetype::FileType;
use crate::swift::Bucket;
use crate::utils::humanise_bytes;
use filetime::{set_file_mtime, FileTime};
use rand::{distributions::Alphanumeric, Rng};
use fs2::free_space;

async fn download_file(data_hash: &str, destination: PathBuf, bucket: &Bucket, data_prefix: &str, cert: &Cert, cache: &PathBuf, hmac_secret: &String) {
  // todo: avoid repeating downloads
  // todo: verify hash matches expected
  
  // this is to avoid the risk of two threads downloading the same data blog at the same time and stepping on eachothers toes.
  // based on the highly scientific process of restoring a backup 10 times and seeing a race condition happen >5 times, and restoring 10 times with
  // no errors after this change, it seems to work.
  let random_suffix: String = rand::thread_rng() 
    .sample_iter(&Alphanumeric)
    .take(4)
    .map(char::from)
    .collect();
  let destination_filename = cache.join(format!("{}{}.gpg", data_hash, random_suffix)); // todo: probably need to do more to guarantee there isn't also a file with this name in the backup
  trace!("attempting to create {:?}", destination_filename);
  let encrypted_file = File::create(&destination_filename).unwrap();
  let key = format!("{}{}", data_prefix, data_hash);
  trace!("downloading {:?}", destination_filename);

  bucket.download(key.as_str(), encrypted_file).await.unwrap();
  trace!("downloaded {:?}", destination_filename);
  let mut source = File::open(&destination_filename).unwrap();
  let mut dest = File::create(&destination).unwrap();
  decryption::decrypt_file(&mut source, &mut dest, cert, None).unwrap();
  if hash::data(&destination, hmac_secret) != data_hash { // todo: do not put file in correct location until this check is done
    std::fs::remove_file(&destination).unwrap();
    panic!("Data hash did not match for {:?}", destination);
  }
  trace!("decrypted {:?}", destination);
}

pub async fn process_file(entry: &FileMetadata, destination: PathBuf, data_bucket: &Bucket, data_prefix: &str, data_cache: &PathBuf, key: &Cert, hmac_secret: &String) -> i64 {
  let canonical_name =  Path::new(entry.name.as_str());
  let suffix = canonical_name.strip_prefix("/").unwrap();
  let path = destination.join(suffix);
  if !path.starts_with(&destination) { //canonicalize() ???
    trace!("ignoring file that is attempting to breach restore path"); // this might require more thought since we allow symlinks
    //continue;
    0
  } else {
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
          hmac_secret
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
        // todo?
        //let permissions = PermissionsExt::from_mode(entry.mode);
        //set_permissions(&path, permissions).unwrap();
        1
      }
      FileType::DIRECTORY => {
        trace!("Creating dir {:?}", &path);
        create_dir_all(path.as_path()).unwrap();
        let permissions = PermissionsExt::from_mode(entry.mode);
        set_permissions(&path, permissions).unwrap();
        1
      }
    }
  }
}

pub async fn restore_backup(destination: PathBuf, backup: &String, store: &DataStore, key_file: PathBuf, hmac_secret: &String, signing_key_file: &Option<PathBuf>) {

  if destination.exists() {
    error!("Bailing because destination already exists");
    return;
  }

  // todo: same progress meters as backup
  // todo: should we make this filename less likely to be something that might be included in an actual backup? Random perhaps?
  let temporary_data_dir = destination.join(".data");
  create_dir_all(&temporary_data_dir).unwrap();

  let key = &Cert::from_file(key_file).unwrap();
  
  let metadata_file = destination.join(".data/metadata.sqlite");

  {
    let encrypted_metadata_file = destination.join(".data/metadata");
    
    trace!("creating {:?}", encrypted_metadata_file);
    
    {
      let encrypted_file = File::create(&encrypted_metadata_file).unwrap();
      let metadata_bucket = store.metadata_bucket().await;
      let prefix = &store.metadata_prefix;
      metadata_bucket.download(format!("{prefix}{backup}.metadata").as_str(), encrypted_file).await.unwrap();
    }
    
    {
      let mut source = File::open(&encrypted_metadata_file).unwrap();
      let mut dest = File::create(&metadata_file).unwrap();
      let signing_key = signing_key_file.clone().map(|x| Cert::from_file(x).unwrap());
      decryption::decrypt_file(&mut source, &mut dest, &key, signing_key).unwrap();
    }
  }

  let metadata_reader = crate::metadata_file::MetadataReader::new(metadata_file).await;

  let size: u64 = metadata_reader.read_metadata("size").await.parse().unwrap();
  info!("Backup is {}", humanise_bytes(size));

  let available_space = free_space(destination.as_path()).unwrap();
  if available_space < size {
    panic!("Backup is {} but disk only has {} available space", humanise_bytes(size), humanise_bytes(available_space));
  }

  let data_cache = &destination.join(".data");
  trace!("Destination: {:?}", destination.as_path());
  let destination = &destination;
  let data_bucket = &store.init().await;
  metadata_reader.read().await
    .map(|entry| async move {
      process_file(&entry, destination.clone(), &data_bucket, &store.data_prefix, &data_cache, &key, hmac_secret).await
    })
    .buffer_unordered(4)
    .count()
    .await;
  remove_dir_all(&temporary_data_dir).unwrap();
}