.PHONY: build test scan query install install-plugin fmt lint check

build:
	cargo build --release

test:
	cargo test

fmt:
	cargo fmt

lint:
	cargo clippy -- -D warnings

check: fmt lint test

install: build
	cargo install --path .

scan: build
	cargo run --release --bin diskcopilot-cli -- scan $(path)

query: build
	cargo run --release --bin diskcopilot-cli -- query large-files $(path) --limit 20

install-plugin:
	mkdir -p ~/.config/yazi/plugins
	ln -sf $(PWD)/diskcopilot.yazi ~/.config/yazi/plugins/diskcopilot.yazi
