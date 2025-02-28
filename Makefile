.PHONY: default dev fmt lint test

fmt:
	cargo fmt

lint:
	cargo clippy -- --deny warnings

test:
	cargo test
