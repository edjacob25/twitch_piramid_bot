FROM docker.io/library/rust:latest as builder
WORKDIR /usr/src/myapp
RUN apt update && apt install -y librocksdb-dev clang libsqlite3-dev
COPY . .
RUN cargo build --release --bin twitch_piramid_bot
FROM docker.io/library/debian:bookworm-slim
RUN apt-get update && apt-get install -y curl libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/myapp/target/release/twitch_piramid_bot /usr/local/bin/bot
ENV RUST_LOG=info
CMD ["bot"]