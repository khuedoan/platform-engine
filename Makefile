.PHONY: default dev fmt lint test

dev:
	docker compose up --detach
	cargo run --bin client
	cargo run --bin worker

fmt:
	cargo fmt

lint:
	cargo clippy -- --deny warnings

test:
	cargo test
