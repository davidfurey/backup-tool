use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::fs::File;
use log::trace;
use serde::Serialize;
extern crate rmp_serde as rmps;
use rmps::Serializer;
use crate::filetype;
use filetype::FileType;
use crate::datastore;
use datastore::DataStore;
use chrono::prelude::{Utc, SecondsFormat};
use rand::{distributions::Alphanumeric, Rng};
use crate::encryption;
extern crate sequoia_openpgp as openpgp;
use openpgp::Cert;

#[derive(Debug, Deserialize, Serialize)]
pub struct FileData {
    pub data: Vec<FileMetadata>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileMetadata {
    pub name: String,
    pub mtime: i64,
    pub mode: u32,
    pub ttype: FileType,
    pub destination: Option<String>,
    pub data_hash: Option<String>,
}

// async fn join_all_discard<I, F>(iter: I) -> ()
// where
//     I: IntoIterator<Item = F>,
//     F: Future<Output = ()>

pub async fn write_metadata_file(path: &PathBuf, metadata_rx: Receiver<FileMetadata>, stores: Vec<DataStore>, key: &Cert) {
  let mut vec = Vec::new();

  while let Ok(msg) = metadata_rx.recv() {
      vec.push(msg);
  }

  trace!("Metadata rx finished");
  let data = FileData {
      data: vec,
  };

  let random_suffix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(4)
    .map(char::from)
    .collect();

  let datetime = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

  let name = format!("backup-{}-{}.metadata", datetime, random_suffix);

  let filename = path.join(&name);

  trace!("Filename: {:?}", filename);
  let mut destination = File::create(&filename).unwrap();

  //todo: should be encrypted [done] (and signed?)
  let policy = openpgp::policy::StandardPolicy::new();
  let mut enc = encryption::encryptor(&policy, &mut destination, &key).unwrap();
  data.serialize(&mut Serializer::new(&mut enc)).unwrap();
  enc.finalize().unwrap();

  // let foobar = futures::stream::iter(stores.iter()).map(|store| async {
  //   let x = store.metadata_bucket().await;
  //   let metadata_file = std::fs::File::open(&filename).unwrap();
  //   x.upload(&name, metadata_file).await.unwrap(); // todo
  // }).buffer_unordered(4).count();
  
  // fn require_send(_: impl Send) {}
  // require_send(foobar);
  let x = stores.get(0).unwrap().metadata_bucket().await;
  let metadata_file = std::fs::File::open(&filename).unwrap();
  x.upload(&name, metadata_file).await.unwrap(); // todo
  //foobar.await;
  trace!("Metadata written");
}

pub async fn write_metadata_file2(path: &PathBuf, vec: Vec<FileMetadata>, stores: Vec<DataStore>, key: &Cert) {
  let data = FileData {
      data: vec,
  };

  let random_suffix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(4)
    .map(char::from)
    .collect();

  let datetime = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

  let name = format!("backup-{}-{}.metadata", datetime, random_suffix);

  let filename = path.join(&name);

  trace!("Filename: {:?}", filename);
  let mut destination = File::create(&filename).unwrap();

  //todo: should be encrypted [done] (and signed?)
  let policy = openpgp::policy::StandardPolicy::new();
  let mut enc = encryption::encryptor(&policy, &mut destination, &key).unwrap();
  data.serialize(&mut Serializer::new(&mut enc)).unwrap();
  enc.finalize().unwrap();

  // let foobar = futures::stream::iter(stores.iter()).map(|store| async {
  //   let x = store.metadata_bucket().await;
  //   let metadata_file = std::fs::File::open(&filename).unwrap();
  //   x.upload(&name, metadata_file).await.unwrap(); // todo
  // }).buffer_unordered(4).count();
  
  // fn require_send(_: impl Send) {}
  // require_send(foobar);
  let x = stores.get(0).unwrap().metadata_bucket().await;
  let metadata_file = std::fs::File::open(&filename).unwrap();
  x.upload(&name, metadata_file).await.unwrap(); // todo
  //foobar.await;
  trace!("Metadata written");
}