FROM docker.io/library/rust:alpine as builder
WORKDIR /usr/src/myapp
RUN apk add --no-cache musl-dev
COPY . .
RUN cargo build --release --bin twitch_piramid_bot
FROM docker.io/library/alpine:latest
COPY --from=builder /usr/src/myapp/target/release/twitch_piramid_bot /usr/local/bin/bot
COPY --from=builder /usr/src/myapp/templates /templates
ENV RUST_LOG=info
CMD ["bot"]
