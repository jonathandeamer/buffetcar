.PHONY: check fmt clippy test deny hooks

check: fmt clippy test ## run the full local gate (fmt, clippy, test)

fmt: ## verify formatting
	cargo fmt --all --check

clippy: ## lint with warnings denied
	cargo clippy --all-targets -- -D warnings

test: ## run the test suite
	cargo test

deny: ## audit dependencies (needs: cargo install cargo-deny)
	cargo deny check advisories licenses bans sources

hooks: ## install git hooks (commit-msg: Conventional Commits); run once per clone
	git config core.hooksPath .githooks
	@echo "git hooks installed (core.hooksPath -> .githooks)"
