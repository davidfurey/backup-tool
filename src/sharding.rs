use std::sync::mpsc::SendError;
use std::sync::mpsc::channel;
use std::sync::mpsc::Sender;
use std::sync::mpsc::Receiver;

pub struct ShardedChannel<T> {
  tx: Vec<Sender<T>>,
}

// unsafe impl<T: Send> Send for Receiver<T> {}
// unsafe impl<T: Send> Send for Receiver<T> {}

// The send port can be sent from place to place, so long as it
// is not used to send non-sendable things.
//#[stable(feature = "rust1", since = "1.0.0")]
//unsafe impl<T: Send> Send for Sender<T> {}

// unsafe impl<T: Send> Send for ShardedChannel<T> {}

// impl<T> !Sync for ShardedChannel<T> {}

impl<T> ShardedChannel<T> {
  pub fn new<F>(count: u32, f: F) -> ShardedChannel<T> 
    where F: Fn(Receiver<T>) -> ()
  {
    let mut transmitters = Vec::new();
    for i in 1..count {
      let (tx, rx) = channel();
      transmitters.push(tx);
      f(rx);
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
      let mut transmitters = Vec::new();
      for i in &self.tx {
        transmitters.push(i.clone());
      }
      ShardedChannel {
        tx: transmitters
      }
    }
}