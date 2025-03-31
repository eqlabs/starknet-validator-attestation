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
docker run -it --rm \
  -e VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY="0xdeadbeef" \
  eqlabs/starknet-validator-attestation \
  --staking-contract-address 0x034370fc9931c636ab07b16ada82d60f05d32993943debe2376847e0921c1162 \
  --attestation-contract-address 0x04862e05d00f2d0981c4a912269c21ad99438598ab86b6e70d1cee267caaa78d \
  --staker-operational-address 0x02e216b191ac966ba1d35cb6cfddfaf9c12aec4dfe869d9fa6233611bb334ee9 \
  --node-url http://localhost:9545/rpc/v0_8
```

Each CLI option can also be set via environment variables. Please check the output of `starknet-validator-attestation --help` for more information.

The private key of the operational account _must_ be set via the `VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY` environment variable.

Log level defaults to `info`. Verbose logging can be enabled by setting the `RUST_LOG` environment variable to `debug`.


## Monitoring

A metrics endpoint is provided for scraping with Prometheus. By default the endpoint is available at `http://127.0.0.1:9090/metrics`. You can use the `--metrics-address` CLI option to change this address.

Available metrics are:

- `validator_attestation_starknet_latest_block_number`: Latest block number seen by the validator.
- `validator_attestation_current_epoch_id`: ID of the current epoch.
- `validator_attestation_current_epoch_length`: Length of the current epoch.
- `validator_attestation_current_epoch_starting_block_number`: First block number of the current epoch.
- `validator_attestation_current_epoch_assigned_block_number`: Block number to attest in current epoch.
- `validator_attestation_last_attestation_timestamp_seconds`: Timestamp of the last attestation.
- `validator_attestation_attestation_submitted_count`: Number of attestations submitted by the validator.
- `validator_attestation_attestation_failure_count`: Number of attestation transaction submission failures.
- `validator_attestation_attestation_confirmed_count`: Number of attestations confirmed by the network.

The chain ID of the network is exposed as the `network` label on all metrics.


## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0)
