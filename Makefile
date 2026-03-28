.PHONY: build release test lint fmt check clean install e2e agent-linux agent-cache

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

# Build static Linux agent binary (musl) for x86_64 via Docker
agent-linux:
	docker run --rm --platform linux/amd64 \
		-v "$(CURDIR):/src" -w /src \
		rust:1.90-slim \
		sh -c "apt-get update -qq && apt-get install -y -qq musl-tools >/dev/null 2>&1 && \
		       rustup target add x86_64-unknown-linux-musl >/dev/null 2>&1 && \
		       cargo build --release --target x86_64-unknown-linux-musl --bin verg-agent 2>&1 | tail -3"
	@echo "Built: target/x86_64-unknown-linux-musl/release/verg-agent"

# Build and cache the agent binary for auto-download path
agent-cache: agent-linux
	mkdir -p "$(HOME)/Library/Application Support/verg/agents"
	cp target/x86_64-unknown-linux-musl/release/verg-agent \
		"$(HOME)/Library/Application Support/verg/agents/verg-agent-x86_64-unknown-linux-gnu"
	@echo "Cached at ~/Library/Application Support/verg/agents/verg-agent-x86_64-unknown-linux-gnu"
