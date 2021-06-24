LOG_LEVEL ?= wagi=debug
MODULES_TOML ?= examples/modules.toml
MODULE_CACHE ?= _scratch/cache
BINDLE ?= example.com/hello/1.3.3
BINDLE_HOST_URL ?= http://localhost:8080/v1
WAGI_IFACE ?= 127.0.0.1:3000
WAGI_HOST ?= localhost:3000
CERT_NAME ?= ssl-example
TLS_OPTS ?= --tls-cert $(CERT_NAME).crt.pem --tls-key $(CERT_NAME).key.pem

.PHONY: build
build:
	cargo build --release

.PHONY: serve
serve: TLS_OPTS = 
serve: _run

.PHONY: serve-tls
serve-tls: ${CERT_NAME}.crt.pem
serve-tls: _run

.PHONY: _run
_run:
	mkdir -p $(MODULE_CACHE)
	RUST_LOG=$(LOG_LEVEL) cargo run --release -- -c $(MODULES_TOML) --module-cache $(MODULE_CACHE) $(TLS_OPTS)

.PHONY: run-bindle
run-bindle:
	mkdir -p $(MODULE_CACHE)
	RUST_LOG=$(LOG_LEVEL) cargo run --release -- -b $(BINDLE) --module-cache $(MODULE_CACHE) --bindle-server $(BINDLE_HOST_URL) --listen $(WAGI_IFACE) --default-host $(WAGI_HOST)

.PHONY: test
test:
	cargo test

$(CERT_NAME).crt.pem:
	openssl req -newkey rsa:2048 -nodes -keyout $(CERT_NAME).key.pem -x509 -days 365 -out $(CERT_NAME).crt.pem
