.PHONY: build test fmt lint check deploy e2e clean

# Default target
all: build

build:
	cargo build --release --target wasm32v1-none

test:
	cargo test

fmt:
	cargo fmt --all

lint:
	cargo clippy --all -- -D warnings

check: fmt lint test

deploy:
	bash scripts/deploy.sh

e2e:
	bash scripts/e2e.sh

clean:
	cargo clean
