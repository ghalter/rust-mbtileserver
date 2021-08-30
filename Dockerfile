# docker build Dockerfile

FROM rust:latest

RUN mkdir /app
WORKDIR /app

ADD . /app

RUN mkdir /app/data

RUN cargo build --release

EXPOSE 5000

ENTRYPOINT ./target/release/mbtileserver -d ./data/mbtiles