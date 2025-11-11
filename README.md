# Starknet Validator Attestation

This is a tool for attesting validators on Starknet. Implements the attestation specification in [SNIP 28](https://community.starknet.io/t/snip-28-staking-v2-proposal/115250).


## Requirements

- A Starknet node with support for the JSON-RPC 0.9.0 API specification. This tool has been tested with [Pathfinder](https://github.com/eqlabs/pathfinder) v0.20.0.
- Staking contracts set up and registered with Staking v2.
- Sufficient funds in the operational account to pay for attestation transactions.


## Installation

You can either use the Docker image we publish on [GitHub](https://github.com/eqlabs/starknet-validator-attestation/pkgs/container/starknet-validator-attestation), the binaries on our [release page](https://github.com/eqlabs/starknet-validator-attestation/releases/latest) or compile the source code from this repository. Compilation requires Rust 1.91+.


## Running

```shell
docker run -it --rm --network host \
  -e VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY="0xdeadbeef" \
  ghcr.io/eqlabs/starknet-validator-attestation \
  --staker-operational-address 0x02e216b191ac966ba1d35cb6cfddfaf9c12aec4dfe869d9fa6233611bb334ee9 \
  --node-url http://localhost:9545/rpc/v0_9 \
  --local-signer
```

Each CLI option can also be set via environment variables. Please check the output of `starknet-validator-attestation --help` for more information.

Log level defaults to `info`. Verbose logging can be enabled by setting the `RUST_LOG` environment variable to `debug`.


### Signatures

There are two options for signing attestation transactions sent by the tool.

- You can use `--local-signer`. In this case you _must_ set the private key of the operational account in the `VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY` environment variable.
- You can use an external signer implementing a simple HTTP API. Use `--remote-signer-url URL` or set the `VALIDATOR_ATTESTATION_REMOTE_SIGNER_URL` to the URL of the external signer API.

#### External signer API

The API should expose a single `/sign` endpoint:

- POST `/sign`: should return the signature for the chain id and transaction values received as its input. The `transaction` object should follow the [INVOKE_TXN_V3](https://github.com/starkware-libs/starknet-specs/blob/a2d10fc6cbaddbe2d3cf6ace5174dd0a306f4885/api/starknet_api_openrpc.json#L2621) schema from the JSON-RPC specification (the signature field is ignored). Example request body:
  ```json
  {
      "transaction": {
          "type": "INVOKE",
          "sender_address": "0x2e216b191ac966ba1d35cb6cfddfaf9c12aec4dfe869d9fa6233611bb334ee9",
          "calldata": [
              "0x1",
              "0x4862e05d00f2d0981c4a912269c21ad99438598ab86b6e70d1cee267caaa78d",
              "0x37446750a403c1b4014436073cf8d08ceadc5b156ac1c8b7b0ca41a0c9c1c54",
              "0x1",
              "0x614f596b9d8eafbc87a48ff3a2a4bd503762d3f4be7c91cdeb766cf869c2233"
          ],
          "version": "0x3",
          "signature": [],
          "nonce": "0xbf",
          "resource_bounds": {
              "l1_gas": {
                  "max_amount": "0x0",
                  "max_price_per_unit": "0x49f83fa3027b"
              },
              "l1_data_gas": {
                  "max_amount": "0x600",
                  "max_price_per_unit": "0x3948c"
              },
              "l2_gas": {
                  "max_amount": "0x1142700",
                  "max_price_per_unit": "0x33a8f57f9"
              }
          },
          "tip": "0x0",
          "paymaster_data": [],
          "account_deployment_data": [],
          "nonce_data_availability_mode": "L1",
          "fee_data_availability_mode": "L1"
      },
      "chain_id": "0x534e5f5345504f4c4941"
  }
  ```
  Response should contain the signature as an array:
  ```json
  {
      "signature": [
          "0x6a775c4dcc7d1a1b8f23a1ab18d9e080ccb8271a7706296dfbadb3563daedfb",
          "0x43fc38b8fd6b204ee52c3843ce060e94f8ed96355bc479dcb2db1292668ccef"
      ]
  }
  ```

An example implementation of the API is available [here](./examples/signer.rs).

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
- `validator_attestation_attestation_confirmed_count`: Number of attestations submitted that have been confirmed by the network.
- `validator_attestation_attestation_confirmations_observed_count`: Number of total attestation confirmations (includes attestation _not_ submitted by this tool).
- `validator_attestation_missed_epochs_count`: Number of epochs with no successful attestation.
- `validator_attestation_operational_account_balance_strk`: Current STRK token balance of the operational account.

The chain ID of the network is exposed as the `network` label on all metrics.


## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0)
