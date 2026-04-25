.DEFAULT_GOAL := help

.PHONY: build
build: ## Build the Rust frontend and libiperf shim
	cargo build

.PHONY: test
test: ## Run Rust unit tests
	cargo test

.PHONY: fmt
fmt: ## Check Rust formatting
	cargo fmt --check

.PHONY: clean
clean: ## Remove Rust build artifacts
	cargo clean

.PHONY: help
help: ## Show this help message
	@awk 'BEGIN {FS = ":.*##"} /^[a-zA-Z0-9_.-]+:.*##/ { printf "  \033[36m%-8s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
