use serde::Serialize;
//use serde_repr::*;

//#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug)]
//#[repr(u8)]
#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum FileType {
    FILE,
    SYMLINK,
    DIRECTORY,
}

impl FileType {
  pub fn from(t: std::fs::FileType) -> Option<FileType> {
    if t.is_dir() {
        Some(FileType::DIRECTORY)
    } else if t.is_file() {
        Some(FileType::FILE)
    } else if t.is_symlink() {
        Some(FileType::SYMLINK)
    } else {
        None
    }
  }
}