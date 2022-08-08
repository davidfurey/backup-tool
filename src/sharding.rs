use std::sync::mpsc::SendError;
use std::sync::mpsc::channel;
use std::sync::mpsc::Sender;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

pub struct ShardedChannel<T> {
  tx: Vec<Sender<T>>,
}

impl<T> ShardedChannel<T> {
  pub fn new<F>(count: u32, receiver: F) -> ShardedChannel<T> 
    where F: Fn(Receiver<T>) -> JoinHandle<()>
  {
    let mut transmitters = Vec::new();
    for _ in 0..count {
      let (tx, rx) = channel();
      transmitters.push(tx);
      receiver(rx);
    }
    return ShardedChannel { tx: transmitters }
  }

  pub fn send(&self, value: T, hash: &str) -> Result<(), SendError<T>> {
    let x = hash.chars().next().unwrap();
    let index = (x as usize) % self.tx.len();
    self.tx.get(index).unwrap().send(value)
  }
}

impl<T> Clone for ShardedChannel<T> {
    fn clone(&self) -> ShardedChannel<T> {
      let transmitters: Vec<Sender<T>> = self.tx.iter().map(|s| {
        s.clone()
      }).collect();
      ShardedChannel {
        tx: transmitters
      }
    }
}