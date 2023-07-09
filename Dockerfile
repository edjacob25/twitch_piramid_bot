FROM rust:latest as builder
WORKDIR /usr/src/myapp
RUN apt update && apt install -y librocksdb-dev clang
COPY . .
RUN cargo build --release --bin twitch_piramid_bot
FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y curl libssl1.1 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/myapp/target/release/twitch_piramid_bot /usr/local/bin/bot
ENV RUST_LOG=info
CMD ["bot"]