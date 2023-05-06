FROM messense/rust-musl-cross:x86_64-musl as builder

WORKDIR /usr/src/app

COPY . .

RUN cargo build --release


FROM alpine:latest

COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/prmirror .

CMD ["./prmirror"]
