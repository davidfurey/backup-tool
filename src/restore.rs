use std::os::unix::prelude::PermissionsExt;
use std::path::{PathBuf, Path};

use futures::{StreamExt, FutureExt};
use futures::stream::FuturesOrdered;
use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use crate::datastore;
use datastore::DataStore;
extern crate rmp_serde as rmps;
use crate::metadata_file;
use metadata_file::FileData;
use crate::decryption;
use decryption::Decryption;
use std::fs::{File, set_permissions, create_dir_all};
use std::os::unix::fs::symlink;
use crate::filetype;
use filetype::FileType;
use crate::swift::Bucket;
use filetime::{set_file_mtime, FileTime};

async fn download_file(data_hash: String, destination: PathBuf, bucket: &Bucket, data_prefix: String, cert: &Cert, cache: &PathBuf) {
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

pub async fn restore_backup(destination: PathBuf, backup: &String, store: &DataStore, key_file: PathBuf) {
  let metadata_bucket = store.metadata_bucket().await;
  let data_bucket = store.init().await;
  let destination_filename = destination.join(".data/metadata.gpg");
  let data_cache = destination.join(".data");
  println!("creating {:?}", destination_filename);
  let encrypted_file = File::create(&destination_filename).unwrap();
  metadata_bucket.download(backup.as_str(), encrypted_file).await.unwrap();
  
  let mut encrypted_file_read = File::open(&destination_filename).unwrap();
  let key = Cert::from_file(key_file).unwrap();
  let decryption = Decryption::new(key.clone());
  let decrypted = decryption.decrypt(&mut encrypted_file_read);
  let mut decryptor = decrypted;
  let metadata: FileData = rmps::from_read(&mut decryptor).unwrap();

  println!("Destination: {:?}", destination.as_path());
  let mut ft = FuturesOrdered::new();

  for entry in metadata.data {
    let canonical_name =  Path::new(entry.name.as_str());
    let suffix = canonical_name.strip_prefix("/").unwrap();
    let path = destination.join(suffix);
    if !path.starts_with(&destination) { //canonicalize() ???
      println!("ignoring file that is attempting to breach restore path"); // this might require more thought since we allow symlinks
      continue;
    }
    match entry.ttype {
      FileType::FILE => {
        println!("Creating file {:?}", &path);
        let downloaded = download_file(entry.data_hash.unwrap(), path.clone(), &data_bucket, store.data_prefix.clone(), &key, &data_cache);
        let mtime = FileTime::from_unix_time(entry.mtime, 0);
        ft.push(downloaded.map(move |f| {
          set_file_mtime(&path, mtime).unwrap();
          let permissions = PermissionsExt::from_mode(entry.mode);
          set_permissions(&path, permissions).unwrap();
          f
        }));
      }
      FileType::SYMLINK => {
        println!("Creating symlink {:?} -> {:?}", &path, entry.destination);
        symlink(entry.destination.unwrap(), &path).unwrap();
        //let permissions = PermissionsExt::from_mode(entry.mode);
        //set_permissions(&path, permissions).unwrap();
      }
      FileType::DIRECTORY => {
        println!("Creating dir {:?}", &path);
        create_dir_all(path.as_path()).unwrap();
        let permissions = PermissionsExt::from_mode(entry.mode);
        set_permissions(&path, permissions).unwrap();
      }
    };
  }
  ft.collect().await
}