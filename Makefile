.PHONY: default dev fmt lint test

dev:
	docker compose up --detach
	cargo run --bin client
	cargo run --bin worker

fmt:
	cargo fmt

lint:
	cargo clippy -- --deny warnings

testdata:
	git clone https://github.com/khuedoan/horus testdata/gitops
	git clone https://github.com/khuedoan/blog testdata/app

test: testdata
	cargo test
