.PHONY: build release test lint fmt check clean install e2e

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

lint:
	cargo fmt --check
	cargo clippy -- -D warnings

fmt:
	cargo fmt

check: lint test

clean:
	cargo clean

install: release
	cp target/release/verg ~/.local/bin/verg
	cp target/release/verg-agent ~/.local/bin/verg-agent

e2e:
	./e2e/run.sh
