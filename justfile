#!/usr/bin/env just --justfile

release:
  cargo build --release    

format:
  cargo fmt --all
  find . -type f -iname "*.toml" -print0 | xargs -0 taplo format

lint:
  cargo clippy --all --all-features -- -D warnings

lintfix:
  cargo clippy --fix --allow-staged --allow-dirty --all-features
  just format


bin:
  cargo run --bin bin -- arg1

example:
  cargo run --example exname -- arg1