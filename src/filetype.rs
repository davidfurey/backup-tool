use serde::Serialize;

#[derive(Debug, Deserialize, Serialize, Clone, strum::Display, sqlx::Type)]
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