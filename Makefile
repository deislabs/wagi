LOG_LEVEL ?= wagi=debug
MODULES_TOML ?= examples/modules.toml
MODULE_CACHE ?= _scratch/cache
BINDLE ?= example.com/hello/1.0.0

.PHONY: build
build:
	cargo build --release

.PHONY: run
run:
	mkdir -p $(MODULE_CACHE)
	RUST_LOG=$(LOG_LEVEL) cargo run --release -- -c $(MODULES_TOML) --module-cache $(MODULE_CACHE)

.PHONY: run-bindle
run-bindle:
	mkdir -p $(MODULE_CACHE)
	RUST_LOG=$(LOG_LEVEL) cargo run --release -- -b $(BINDLE) --module-cache $(MODULE_CACHE)

.PHONY: test
test:
	cargo test

