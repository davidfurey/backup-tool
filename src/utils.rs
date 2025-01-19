pub fn humanise_bytes(b: u64) -> String {
  if b > 1024*1024*1024 {
    format!("{:.2}GiB", (b as f64) / (1024.0*1024.0*1024.0))
  } else if b > 1024*1024 {
    format!("{:.2}MiB", (b as f64) / (1024.0*1024.0))
  } else if b > 1024 {
    format!("{:.2}KiB", (b as f64) / 1024.0)
  } else {
    format!("{} bytes", b)
  }
}