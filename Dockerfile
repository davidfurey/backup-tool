# syntax=docker/dockerfile:1
FROM rust:1.84.0

COPY ./ /root/backup-tool

RUN apt-get update; \
  apt-get install -y clang llvm pkg-config nettle-dev; \
  cd /root/backup-tool; \
  cargo build -r; \
  cp /root/backup-tool/target/release/backup-tool /usr/local/bin/;

FROM ubuntu:22.04
COPY --from=0 /usr/local/bin/backup-tool /usr/local/bin/backup-tool
ENTRYPOINT ["/usr/local/bin/backup-tool"]

CMD ["help"]
