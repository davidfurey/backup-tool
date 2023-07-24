use std::sync::mpsc::SendError;
use std::sync::mpsc::sync_channel;
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::Receiver;


pub struct ShardedChannel<T> {
  tx: Vec<SyncSender<T>>,
}

impl<T> ShardedChannel<T> {
  pub fn new<F>(count: u32, receiver: F) -> ShardedChannel<T> 
    where F: Fn(Receiver<T>)
  {
    let mut transmitters = Vec::new();
    for _ in 0..count {
      let (tx, rx) = sync_channel(32);
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
      let transmitters: Vec<SyncSender<T>> = self.tx.iter().map(|s| {
        s.clone()
      }).collect();
      ShardedChannel {
        tx: transmitters
      }
    }
}