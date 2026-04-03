.PHONY: build test scan query install-plugin fmt lint check

build:
	cargo build --release

test:
	cargo test

fmt:
	cargo fmt

lint:
	cargo clippy -- -D warnings

check: fmt lint test

scan: build
	cargo run --release -- scan $(path)

query: build
	cargo run --release -- query large-files $(path) --limit 20

install-plugin:
	mkdir -p ~/.config/yazi/plugins
	ln -sf $(PWD)/diskcopilot.yazi ~/.config/yazi/plugins/diskcopilot.yazi
