.PHONY: check test fmt lint ci run release backup

check:
	cargo check

test:
	cargo test

fmt:
	cargo fmt

lint:
	cargo clippy --all-targets -- -D warnings

# Everything CI should run.
ci:
	cargo check
	cargo test
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

run:
	cargo run

release:
	cargo build --release

backup:
	./scripts/backup.sh
