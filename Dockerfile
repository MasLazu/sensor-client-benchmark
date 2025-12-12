FROM rust:1.83 as builder

WORKDIR /usr/src/app
COPY . .

RUN apt-get update && apt-get install -y protobuf-compiler
RUN cargo build --release

FROM debian:bookworm-slim

WORKDIR /usr/local/bin

COPY --from=builder /usr/src/app/target/release/sensor-benchmark-tool .

CMD ["./sensor-benchmark-tool"]
