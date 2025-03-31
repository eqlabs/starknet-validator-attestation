# Starknet Validator Attestation

This is a tool for attesting validators on Starknet. Implements the attestation specification in [SNIP 28](https://community.starknet.io/t/snip-28-staking-v2-proposal/115250).

## Requirements

- A Starknet node with support for the JSON-RPC 0.8.0 API specification. This tool has been tested with [Pathfinder](https://github.com/eqlabs/pathfinder).
- Staking contracts set up and registered with Staking v2.
- Sufficient funds in the operational account to pay for attestation transactions.

## Installation

You can either use the Docker image we publish on [Docker Hub](https://hub.docker.com/r/eqlabs/starknet-validator-attestation) or compile the source code from this repository. Compilation requires Rust 1.85+.

## Running

```shell
docker run -it --rm eqlabs/starknet-validator-attestation \
  --staking-contract-address 0x034370fc9931c636ab07b16ada82d60f05d32993943debe2376847e0921c1162 \
  --attestation-contract-address 0x04862e05d00f2d0981c4a912269c21ad99438598ab86b6e70d1cee267caaa78d \
  --staker-operational-address 0x02e216b191ac966ba1d35cb6cfddfaf9c12aec4dfe869d9fa6233611bb334ee9 \
  --node-url http://localhost:9545/rpc/v0_8
```

## Monitoring

A metrics endpoint is provided for scraping with Prometheus. By default the endpoint is available at `http://127.0.0.1:9090/metrics`. You can use the `--metrics-address` CLI option to change this address.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0)
