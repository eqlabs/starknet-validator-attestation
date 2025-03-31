FROM lukemathwalker/cargo-chef:0.1.71-rust-1.85.1-slim-bookworm AS cargo-chef
WORKDIR /src

FROM cargo-chef AS rust-planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM cargo-chef AS rust-builder
COPY --from=rust-planner /src/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim AS runner
RUN apt-get update && apt-get install -y ca-certificates tini && rm -rf /var/lib/apt/lists/*
RUN groupadd --gid 1000 starknet && useradd --uid 1000 --gid 1000 starknet

COPY --from=rust-builder /src/target/release/starknet-validator-attestation /usr/local/bin/

USER 1000:1000

# Expose metrics API
EXPOSE 9090
ENV VALIDATOR_ATTESTATION_METRICS_ADDRESS="0.0.0.0:9090"

# Default log level
ENV RUST_LOG="info"

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/starknet-validator-attestation"]
CMD []
