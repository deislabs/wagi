LOG_LEVEL ?= info
MODULES_TOML ?= examples/modules.toml

.PHONY: build
build:
	cargo build --release

.PHONY: run
run:
	RUST_LOG=$(LOG_LEVEL) cargo run --release -- -c $(MODULES_TOML)

.PHONY: test
test:
	cargo test

