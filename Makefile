.PHONY: default build dev fix fmt lint test clean

default: build

build:
	cargo build --release

dev:
	docker compose up --detach
	bacon run

fix:
	cargo clippy --fix --allow-staged

fmt:
	cargo fmt

lint:
	cargo clippy -- --deny warnings

testdata:
	git clone https://github.com/khuedoan/cloudlab testdata/cloudlab
	git clone https://github.com/khuedoan/example-service testdata/example-service
	git clone https://github.com/khuedoan/micropaas testdata/micropaas

test: testdata
	cargo nextest run

clean:
	docker compose down --volumes
	rm -rf /tmp/workspace
	docker image remove localhost:5000/test-build-dockerfile:latest
	docker image remove localhost:5000/test-build-nixpacks:latest
