.PHONY: test build fmt clippy check clean

test:
	cargo test

build:
	stellar contract build

fmt:
	cargo fmt

clippy:
	cargo clippy -- -D warnings

check: fmt clippy test build

clean:
	cargo clean
