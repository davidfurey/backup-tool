Backup-tool
===========

Build
------

docker build -t backup-tool .


Debugging with console-subscriber
---------------------------------

RUSTFLAGS="--cfg tokio_unstable" cargo run --features console
