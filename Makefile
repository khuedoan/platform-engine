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
	git clone https://github.com/khuedoan/horus testdata/horus
	git clone https://github.com/khuedoan/blog testdata/blog
	git clone https://github.com/khuedoan/micropaas testdata/micropaas

test: testdata
	cargo test
