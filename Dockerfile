FROM rust:latest as builder
LABEL authors="tomokazu"
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:bullseye-slim
WORKDIR /app
RUN apt-get update && apt-get install -y libssl-dev netcat
COPY startup.sh ./
RUN chmod +x ./startup.sh
COPY --from=builder /app/target/release/radiko-cacher ./
CMD ["./startup.sh"]

