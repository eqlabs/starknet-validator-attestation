default:
    just --summary --unsorted

test $RUST_BACKTRACE="1" *args="":
    cargo test --workspace --all-targets {{args}}

check:
    cargo check --workspace --all-targets --locked

clippy *args="":
    cargo clippy --workspace --all-targets --locked {{args}} -- -D warnings -D rust_2018_idioms

fmt:
    cargo fmt  --all
