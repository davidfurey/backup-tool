use std::os::unix::prelude::PermissionsExt;
use std::path::{PathBuf, Path};

use futures::{StreamExt, FutureExt};
use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use crate::datastore;
use datastore::DataStore;
extern crate rmp_serde as rmps;
use crate::metadata_file::{self, FileMetadata};
use metadata_file::FileData;
use crate::decryption;
use decryption::Decryption;
use std::fs::{File, set_permissions, create_dir_all, remove_dir_all};
use std::os::unix::fs::symlink;
use crate::filetype;
use filetype::FileType;
use crate::swift::Bucket;
use filetime::{set_file_mtime, FileTime};

async fn download_file(data_hash: &str, destination: PathBuf, bucket: &Bucket, data_prefix: &str, cert: &Cert, cache: &PathBuf) {
  // todo: avoid repeating downloads
  let destination_filename = cache.join(format!("{}.gpg", data_hash));
  println!("attempting to create {:?}", destination_filename);
  let encrypted_file = File::create(&destination_filename).unwrap();
  let key = format!("{}{}", data_prefix, data_hash);
  println!("downloading {:?}", destination_filename);

  bucket.download(key.as_str(), encrypted_file).await.unwrap();
  println!("downloaded {:?}", destination_filename);
  let mut source = File::open(&destination_filename).unwrap();
  let mut dest = File::create(&destination).unwrap();
  decryption::decrypt_file(&mut source, &mut dest, cert).unwrap();
  println!("decrypted {:?}", destination);
}

pub async fn process_file(entry: &FileMetadata, destination: PathBuf, data_bucket: &Bucket, data_prefix: &str, data_cache: &PathBuf, key: &Cert) -> i64 {
  let canonical_name =  Path::new(entry.name.as_str());
  let suffix = canonical_name.strip_prefix("/").unwrap();
  let path = destination.join(suffix);
  if !path.starts_with(&destination) { //canonicalize() ???
    println!("ignoring file that is attempting to breach restore path"); // this might require more thought since we allow symlinks
    //continue;
    0
  } else {
    match entry.ttype {
      FileType::FILE => {
        println!("Creating file {:?}", &path);
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
          &data_cache
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
        println!("Creating symlink {:?} -> {:?}", &path, entry.destination);
        symlink(entry.destination.clone().unwrap(), &path).unwrap();
        // todo?
        //let permissions = PermissionsExt::from_mode(entry.mode);
        //set_permissions(&path, permissions).unwrap();
        1
      }
      FileType::DIRECTORY => {
        println!("Creating dir {:?}", &path);
        create_dir_all(path.as_path()).unwrap();
        let permissions = PermissionsExt::from_mode(entry.mode);
        set_permissions(&path, permissions).unwrap();
        1
      }
    }
  }
}

pub async fn restore_backup(destination: PathBuf, backup: &String, store: &DataStore, key_file: PathBuf) {

  if destination.exists() {
    println!("Bailing because destination already exists");
    return;
  }

  // todo: should we make this filename less likely to be something that might be included in an actual backup? Random perhaps?
  let temporary_data_dir = destination.join(".data");
  create_dir_all(&temporary_data_dir).unwrap();

  let metadata_bucket = store.metadata_bucket().await;
  let data_bucket = store.init().await;
  let destination_filename = destination.join(".data/metadata.gpg");
  let data_cache = destination.join(".data");
  println!("creating {:?}", destination_filename);
  let encrypted_file = File::create(&destination_filename).unwrap();
  metadata_bucket.download(format!("{backup}.metadata").as_str(), encrypted_file).await.unwrap();
  
  let mut encrypted_file_read = File::open(&destination_filename).unwrap();
  let key = Cert::from_file(key_file).unwrap();
  let decryption = Decryption::new(key.clone());
  let decrypted = decryption.decrypt(&mut encrypted_file_read);
  let mut decryptor = decrypted;
  let metadata: FileData = rmps::from_read(&mut decryptor).unwrap();

  println!("Destination: {:?}", destination.as_path());
  futures::stream::iter(metadata.data.iter())
    .map(|entry| async {
      process_file(entry, destination.clone(), &data_bucket, &store.data_prefix, &data_cache, &key).await
    })
    .buffer_unordered(4)
    .count()
    .await;
  remove_dir_all(&temporary_data_dir).unwrap();
}