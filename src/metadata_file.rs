use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::fs::File;
use serde::Serialize;
extern crate rmp_serde as rmps;
use rmps::Serializer;
use crate::filetype;
use filetype::FileType;
use crate::datastore;
use datastore::DataStore;
use chrono::prelude::{Utc, SecondsFormat};
use rand::{distributions::Alphanumeric, Rng};

#[derive(Debug, Deserialize, Serialize)]
pub struct FileData {
    data: Vec<FileMetadata>,
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

pub fn write_metadata_file(path: &PathBuf, metadata_rx: Receiver<FileMetadata>, stores: Arc<Vec<DataStore>>) {
  let mut vec = Vec::new();

  while let Ok(msg) = metadata_rx.recv() {
      vec.push(msg);
  }

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

  let filename = path.join(name);

  let destination = File::create(&filename).unwrap();

  //todo: should be encrypted (and signed?)
  data.serialize(&mut Serializer::new(destination)).unwrap();

  for store in stores.iter() {
    let metadata_file = std::fs::File::open(&filename).unwrap();
    store.metadata_bucket().upload("foo", metadata_file).unwrap();
  }
}