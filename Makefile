LOG_LEVEL ?= wagi=debug
MODULES_TOML ?= examples/modules.toml
MODULE_CACHE ?= _scratch/cache

.PHONY: build
build:
	cargo build --release

.PHONY: run
run:
	mkdir -p $(MODULE_CACHE)
	RUST_LOG=$(LOG_LEVEL) cargo run --release -- -c $(MODULES_TOML) --module-cache $(MODULE_CACHE)

.PHONY: test
test:
	cargo test

