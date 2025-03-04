.PHONY: default dev fmt lint test clean

dev:
	docker compose up --detach
	bacon run

fmt:
	cargo clippy --fix --allow-staged
	cargo fmt

lint:
	cargo clippy -- --deny warnings

testdata:
	git clone https://github.com/khuedoan/horus testdata/horus
	git clone https://github.com/khuedoan/example-service testdata/example-service
	git clone https://github.com/khuedoan/micropaas testdata/micropaas

test: testdata
	bacon test

clean:
	rm -rf /tmp/workspace
	docker image remove localhost/test-build-docker:latest
	docker image remove localhost/test-build-nixpacks:latest
