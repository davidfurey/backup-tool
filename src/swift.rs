use std::fs::File;
use osauth::Session;
use osauth::services::OBJECT_STORAGE;
use futures::stream::StreamExt;
use tokio_util::io::StreamReader;
use crate::query;
use query::Query;

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct ObjectEntry {
  hash: String,
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
            //session: osauth::Session::from_env().unwrap(),
        }
    }

    pub async fn upload(&self, key: &str, source: File) -> Result<reqwest::Response, osauth::Error> {
      let tokio_file = tokio::fs::File::from(source);
      return self.session.put(OBJECT_STORAGE, &[self.container.as_ref(), key])
        .body(tokio_file)
        .send().await;
    }

    pub async fn download(&self, key: &str, dest: File) -> std::io::Result<u64> {
      let response = self.session.get(OBJECT_STORAGE, &[self.container.as_ref(), key]).send().await.unwrap();
      let stream = response
        .bytes_stream()
        .map(|result| {
            result.map_err(|_error| {
                println!("Encountered error");
                std::io::Error::new(std::io::ErrorKind::Other, "Error!")
              }
            )
        });
      let mut reader = StreamReader::new(stream);
      let mut tokio_file = tokio::fs::File::from(dest);
      tokio::io::copy(&mut reader, &mut tokio_file).await
    }

    pub async fn list(&self, key: &str) -> std::io::Result<Vec<ObjectEntry>> {
      let mut query = Query::new();
      query.push_str("format", "json");
      let response = self.session.get(OBJECT_STORAGE, &[self.container.as_ref(), key])
        .query(&query)
        .send().await.unwrap()
        .json().await.unwrap();
      Ok(response)
    }
}
