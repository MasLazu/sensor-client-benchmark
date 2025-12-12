FROM rust:1.83 as builder

WORKDIR /usr/src/app
COPY ./sensor-benchmark-tool .
COPY ./sensor-suricata-service-rust/proto ./proto

RUN apt-get update && apt-get install -y protobuf-compiler
RUN cargo build --release

FROM debian:bookworm-slim

WORKDIR /usr/local/bin

COPY --from=builder /usr/src/app/target/release/sensor-benchmark-tool .

CMD ["./sensor-benchmark-tool"]
