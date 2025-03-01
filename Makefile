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
	git clone https://github.com/khuedoan/example-service testdata/example-service
	git clone https://github.com/khuedoan/micropaas testdata/micropaas

test: testdata
	cargo test
	rm -rf /tmp/workspace
	docker image remove localhost/test-build-docker:latest
	docker image remove localhost/test-build-nixpacks:latest
