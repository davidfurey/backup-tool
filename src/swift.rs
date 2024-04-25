use std::fs::File;
use futures::TryStreamExt;
use log::error;
use osauth::Session;
use osauth::services::OBJECT_STORAGE;
use futures::stream::StreamExt;
use tokio_util::io::StreamReader;
use crate::query;
use query::Query;

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct ObjectEntry {
  pub hash: String,
  last_modified: String,
  bytes: i128,
  pub name: String,
  content_type: String,
}

pub struct Bucket {
  session: Session,
  container: String,
}

impl Bucket {
    pub fn new(session: Session, container: &str) -> Bucket {
        Bucket {
            session,
            container: container.to_string(),
        }
    }

    pub async fn upload(&self, key: &str, source: File) -> Result<reqwest::Response, osauth::Error> {
      let tokio_file = tokio::fs::File::from(source);
      return self.session.put(OBJECT_STORAGE, &[self.container.as_ref(), key])
        .body(tokio_file)
        .send().await;
    }

    pub async fn upload_with_progress(&self, key: &str, source: File, callback: impl Fn(usize) + Sync + Send + 'static) -> Result<reqwest::Response, osauth::Error> {
      let tokio_file = tokio::fs::File::from(source);
      let stream = tokio_util::io::ReaderStream::new(tokio_file).inspect_ok(move |bytes| {
        callback(bytes.len())
      });
      return self.session.put(OBJECT_STORAGE, &[self.container.as_ref(), key])
        .body(reqwest::Body::wrap_stream(stream))
        .send().await;
    }

    pub async fn download(&self, key: &str, dest: File) -> std::io::Result<u64> {
      let response = self.session.get(OBJECT_STORAGE, &[self.container.as_ref(), key]).send().await.unwrap();
      let stream = response
        .bytes_stream()
        .map(|result| {
            result.map_err(|_error| {
                error!("Encountered error");
                std::io::Error::new(std::io::ErrorKind::Other, "Error!")
              }
            )
        });
      let mut reader = StreamReader::new(stream);
      let mut tokio_file = tokio::fs::File::from(dest);
      tokio::io::copy(&mut reader, &mut tokio_file).await
    }

    pub async fn list(&self, prefix: Option<&str>, marker: Option<&str>) -> std::io::Result<Vec<ObjectEntry>> {
      let mut query = Query::new();
      query.push_str("format", "json");
      query.push_str("limit", "100");
      match prefix {
        Some(p) => {
          query.push_str("prefix", p);
        },
        _ => {}
      };
      match marker {
        Some(m) => {
          query.push_str("marker", m);
        },
        _ => {}
      };

      let response = self.session.get(OBJECT_STORAGE, &[self.container.as_ref(), ""])
        .query(&query)
        .send().await.unwrap()
        .json().await.unwrap();
      Ok(response)
    }
}
