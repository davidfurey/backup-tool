use std::sync::mpsc::Receiver;
use std::collections::HashMap;
use std::fs::File;

use serde::Serialize;
use rmps::Serializer;

use crate::FileType;

#[derive(Debug, Deserialize, Serialize)]
pub struct FileData {
    data: Vec<FileMetadata>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileMetadata {
    pub name: String,
    pub mtime: i64,
    pub mode: u32,
    pub xattr: HashMap<String, String>,
    pub ttype: FileType,
    pub destination: Option<String>,
    pub data_hash: Option<String>,
}

pub fn write_metadata_file(metadata_rx: Receiver<FileMetadata>) {
  let mut vec = Vec::new();

  while let Ok(msg) = metadata_rx.recv() {
      vec.push(msg);
  }

  let data = FileData {
      data: vec,
  };

  let destination = File::create("test.dat").unwrap();

  data.serialize(&mut Serializer::new(destination)).unwrap();
}