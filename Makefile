SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c
.DEFAULT_GOAL := help

CARGO      := $(shell if command -v rustup >/dev/null 2>&1 && rustup which cargo --toolchain stable >/dev/null 2>&1; then rustup which cargo --toolchain stable; else command -v cargo; fi)
RUSTC      := $(shell if command -v rustup >/dev/null 2>&1 && rustup which rustc --toolchain stable >/dev/null 2>&1; then rustup which rustc --toolchain stable; else command -v rustc; fi)
RUSTDOC    := $(shell if command -v rustup >/dev/null 2>&1 && rustup which rustdoc --toolchain stable >/dev/null 2>&1; then rustup which rustdoc --toolchain stable; else command -v rustdoc; fi)
RUSTUP     ?= rustup
RUSTUP_TOOLCHAIN ?= stable
CARGO_ENV  := RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)"
GIT        ?= git
GH         ?= gh
DOCKER     ?= docker
REMOTE     ?= origin
MAIN_BRANCH ?= main

APP        := iperf3-rs
BINDIR     := bin
DISTDIR    := dist
VERSION    := $(shell awk 'BEGIN { in_pkg = 0 } /^\[package\]$$/ { in_pkg = 1; next } /^\[/ { in_pkg = 0 } in_pkg && $$1 == "version" { gsub(/"/, "", $$3); print $$3; exit }' Cargo.toml)
TAG        ?= v$(VERSION)
OS         ?= darwin,linux
ARCH       ?= amd64,arm64

DARWIN_ARCHS := amd64 arm64
LINUX_ARCHS  := amd64 arm64
RUST_TARGETS := x86_64-apple-darwin aarch64-apple-darwin

DARWIN_amd64_TARGET := x86_64-apple-darwin
DARWIN_amd64_SUFFIX := darwin-amd64
DARWIN_arm64_TARGET := aarch64-apple-darwin
DARWIN_arm64_SUFFIX := darwin-arm64

LINUX_amd64_PLATFORM := linux/amd64
LINUX_amd64_SUFFIX := linux-amd64
LINUX_arm64_PLATFORM := linux/arm64
LINUX_arm64_SUFFIX := linux-arm64
LINUX_BUILD_IMAGE ?= rust:1.95-bookworm
DOCKER_UID ?= $(shell id -u)
DOCKER_GID ?= $(shell id -g)
HOST_OS := $(shell uname -s)

RELEASE_CONFIGURE_ARGS ?= --without-openssl
MAIN_REMOTE_REF := refs/remotes/$(REMOTE)/$(MAIN_BRANCH)

##@ Development

.PHONY: build
build: ## Build the host binary into bin/
	@mkdir -p $(BINDIR)
	@$(CARGO_ENV) $(CARGO) build --release
	@cp target/release/$(APP) $(BINDIR)/$(APP)
	@chmod +x $(BINDIR)/$(APP)
	@printf 'Wrote %s/%s\n' "$(BINDIR)" "$(APP)"

.PHONY: fmt
fmt: ## Format Rust sources
	@$(CARGO_ENV) $(CARGO) fmt --all

.PHONY: fmt-check
fmt-check: ## Check Rust formatting without changing files
	@$(CARGO_ENV) $(CARGO) fmt --all --check

.PHONY: lint
lint: ## Run clippy with warnings treated as errors
	@$(CARGO_ENV) $(CARGO) clippy --all-targets --all-features -- -D warnings

.PHONY: test
test: ## Run unit tests
	@$(CARGO_ENV) $(CARGO) test

.PHONY: check
check: fmt-check lint test ## Run formatting, lint, and tests

.PHONY: clean
clean: ## Remove local build artifacts
	@rm -rf $(BINDIR) $(DISTDIR) .cargo-linux .home-linux
	@$(CARGO_ENV) $(CARGO) clean

##@ Distribution

.PHONY: dist
dist: ## Build release binaries into dist/. Use OS=darwin,linux and ARCH=amd64,arm64
	@rm -rf $(DISTDIR)
	@mkdir -p $(DISTDIR)
	@os_list="$(OS)"; \
	arch_list="$(ARCH)"; \
	if [ -z "$$os_list" ]; then \
		echo "OS is required. Supported values: darwin,linux" >&2; \
		exit 1; \
	fi; \
	if [ -z "$$arch_list" ]; then \
		echo "ARCH is required. Supported values: amd64,arm64" >&2; \
		exit 1; \
	fi; \
	for os in $$(printf '%s' "$$os_list" | tr ',' ' '); do \
		case "$$os" in \
			darwin|linux) ;; \
			*) echo "Unsupported OS '$$os'. Supported values: darwin,linux" >&2; exit 1 ;; \
		esac; \
	done; \
	for arch in $$(printf '%s' "$$arch_list" | tr ',' ' '); do \
		case "$$arch" in \
			amd64|arm64) ;; \
			*) echo "Unsupported ARCH '$$arch'. Supported values: amd64,arm64" >&2; exit 1 ;; \
		esac; \
	done; \
	for os in $$(printf '%s' "$$os_list" | tr ',' ' '); do \
		for arch in $$(printf '%s' "$$arch_list" | tr ',' ' '); do \
			$(MAKE) _dist.$$os.$$arch || exit $$?; \
		done; \
	done; \
	$(MAKE) checksums

.PHONY: checksums
checksums: ## Write SHA-256 checksums for dist artifacts
	@if [ ! -d "$(DISTDIR)" ] || ! ls "$(DISTDIR)"/$(APP)-* >/dev/null 2>&1; then \
		echo "No dist artifacts found" >&2; \
		exit 1; \
	fi
	@cd "$(DISTDIR)" && shasum -a 256 $(APP)-* > checksums.txt
	@printf 'Wrote %s/checksums.txt\n' "$(DISTDIR)"

.PHONY: _docker-check
_docker-check:
	@command -v $(DOCKER) >/dev/null 2>&1 || { \
		echo "Docker is required for Linux release builds" >&2; \
		exit 1; \
	}
	@$(DOCKER) info >/dev/null 2>&1 || { \
		echo "A running Docker daemon is required for Linux release builds" >&2; \
		exit 1; \
	}

define TARGET_RULE
.PHONY: _target.$(1)
_target.$(1):
	@command -v $(RUSTUP) >/dev/null 2>&1 || { \
		echo "rustup is required to install cross-compilation targets" >&2; \
		exit 1; \
	}
	@$(RUSTUP) target add --toolchain $(RUSTUP_TOOLCHAIN) $(1)
endef
$(foreach target,$(RUST_TARGETS),$(eval $(call TARGET_RULE,$(target))))

define DARWIN_DIST_RULE
.PHONY: _dist.darwin.$(1)
_dist.darwin.$(1): _target.$$(DARWIN_$(1)_TARGET)
	@if [ "$(HOST_OS)" != "Darwin" ]; then \
		echo "Darwin release builds must run on macOS" >&2; \
		exit 1; \
	fi
	@printf 'Building %s for %s\n' "$(APP)" "$$(DARWIN_$(1)_TARGET)"
	@mkdir -p $(DISTDIR)
	@IPERF3_RS_CONFIGURE_ARGS="$(RELEASE_CONFIGURE_ARGS)" \
		$(CARGO_ENV) $(CARGO) build --release --target $$(DARWIN_$(1)_TARGET)
	@cp target/$$(DARWIN_$(1)_TARGET)/release/$(APP) $(DISTDIR)/$(APP)-$$(DARWIN_$(1)_SUFFIX)
	@chmod +x $(DISTDIR)/$(APP)-$$(DARWIN_$(1)_SUFFIX)
	@printf 'Wrote %s/%s-%s\n' "$(DISTDIR)" "$(APP)" "$$(DARWIN_$(1)_SUFFIX)"
endef
$(foreach arch,$(DARWIN_ARCHS),$(eval $(call DARWIN_DIST_RULE,$(arch))))

define LINUX_DIST_RULE
.PHONY: _dist.linux.$(1)
_dist.linux.$(1): _docker-check
	@printf 'Building %s for %s via Docker\n' "$(APP)" "$$(LINUX_$(1)_PLATFORM)"
	@mkdir -p $(DISTDIR) .cargo-linux/$(1) .home-linux/$(1)
	@$(DOCKER) run --rm \
		--platform $$(LINUX_$(1)_PLATFORM) \
		-e HOME=/workspace/.home-linux/$(1) \
		-e CARGO_HOME=/workspace/.cargo-linux/$(1) \
		-e CARGO_TARGET_DIR=/workspace/target/linux-$(1) \
		-e IPERF3_RS_CONFIGURE_ARGS="$(RELEASE_CONFIGURE_ARGS)" \
		-v "$(CURDIR):/workspace" \
		-w /workspace \
		$(LINUX_BUILD_IMAGE) \
		bash -eu -o pipefail -c ' \
			apt-get update >/dev/null; \
			apt-get install -y --no-install-recommends build-essential make pkg-config ca-certificates >/dev/null; \
			cargo build --release; \
			cp target/linux-$(1)/release/$(APP) dist/$(APP)-$$(LINUX_$(1)_SUFFIX); \
			chmod +x dist/$(APP)-$$(LINUX_$(1)_SUFFIX); \
			chown -R $(DOCKER_UID):$(DOCKER_GID) dist target/linux-$(1) .cargo-linux/$(1) .home-linux/$(1)'
	@printf 'Wrote %s/%s-%s\n' "$(DISTDIR)" "$(APP)" "$$(LINUX_$(1)_SUFFIX)"
endef
$(foreach arch,$(LINUX_ARCHS),$(eval $(call LINUX_DIST_RULE,$(arch))))

##@ Release

.PHONY: _publish-release
_publish-release:
	@command -v $(GH) >/dev/null 2>&1 || { \
		echo "gh is required to publish a release" >&2; \
		exit 1; \
	}
	@if [ -z "$(TAG)" ]; then \
		echo "TAG is required for the release upload step" >&2; \
		exit 1; \
	fi
	@if [ -z "$(TARGET_SHA)" ]; then \
		echo "TARGET_SHA is required for the release upload step" >&2; \
		exit 1; \
	fi
	@if $(GH) release view "$(TAG)" >/dev/null 2>&1; then \
		echo "Release $(TAG) already exists" >&2; \
		exit 1; \
	fi
	@if $(GIT) ls-remote --exit-code --tags "$(REMOTE)" "refs/tags/$(TAG)" >/dev/null 2>&1; then \
		echo "Tag $(TAG) already exists on $(REMOTE)" >&2; \
		exit 1; \
	fi
	@if ! ls "$(DISTDIR)"/$(APP)-* "$(DISTDIR)"/checksums.txt >/dev/null 2>&1; then \
		echo "No release assets found in $(DISTDIR). Run make dist first." >&2; \
		exit 1; \
	fi
	@printf 'Creating GitHub release %s at %s\n' "$(TAG)" "$(TARGET_SHA)"
	@$(GH) release create "$(TAG)" "$(DISTDIR)"/$(APP)-* "$(DISTDIR)"/checksums.txt \
		--target "$(TARGET_SHA)" \
		--title "$(TAG)" \
		--notes "Release $(TAG) built from $(TARGET_SHA)"

.PHONY: release
release: ## Build all binaries for the version on origin/main and publish a GitHub Release
	@command -v $(GIT) >/dev/null 2>&1 || { \
		echo "git is required to create a release" >&2; \
		exit 1; \
	}
	@make_bin="$$(command -v make)"; \
	tmpdir="$$(mktemp -d)"; \
	main_ref="$(MAIN_REMOTE_REF)"; \
	trap 'status=$$?; $(GIT) worktree remove --force "$$tmpdir" >/dev/null 2>&1 || true; rm -rf "$$tmpdir"; exit $$status' EXIT; \
	printf 'Fetching %s/%s\n' "$(REMOTE)" "$(MAIN_BRANCH)"; \
	$(GIT) fetch $(REMOTE) $(MAIN_BRANCH); \
	main_sha="$$($(GIT) rev-parse "$$main_ref")"; \
	printf 'Preparing release worktree for %s\n' "$$main_sha"; \
	$(GIT) worktree add --force --detach "$$tmpdir" "$$main_sha" >/dev/null; \
	$(GIT) -C "$$tmpdir" submodule update --init --recursive; \
	release_version="$$(awk 'BEGIN { in_pkg = 0 } /^\[package\]$$/ { in_pkg = 1; next } /^\[/ { in_pkg = 0 } in_pkg && $$1 == "version" { gsub(/"/, "", $$3); print $$3; exit }' "$$tmpdir/Cargo.toml")"; \
	if [ -z "$$release_version" ]; then \
		echo "failed to read package.version from $$tmpdir/Cargo.toml" >&2; \
		exit 1; \
	fi; \
	tag="v$$release_version"; \
	printf 'Building release assets for %s\n' "$$tag"; \
	"$$make_bin" -f "$(CURDIR)/Makefile" -C "$$tmpdir" dist OS=darwin,linux ARCH=amd64,arm64; \
	printf 'Publishing %s\n' "$$tag"; \
	"$$make_bin" -f "$(CURDIR)/Makefile" -C "$$tmpdir" _publish-release TAG="$$tag" TARGET_SHA="$$main_sha"

##@ Help

.PHONY: help
help: ## Show this help message
	@awk 'BEGIN {FS = ":.*##"; section = ""} \
	/^[a-zA-Z0-9_.-]+:.*##/ { \
		if (section != "") printf "\n\033[1m%s\033[0m\n", section; \
		section = ""; \
		printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2; next \
	} \
	/^##@/ { section = substr($$0, 5); next }' $(MAKEFILE_LIST)
	@printf "\n\033[1mVariables:\033[0m\n"
	@printf "  \033[36mOS\033[0m       Release OS list: \033[36mdarwin,linux\033[0m\n"
	@printf "  \033[36mARCH\033[0m     Release arch list: \033[36mamd64,arm64\033[0m\n"
	@printf "  \033[36mTAG\033[0m      GitHub release tag, defaults to \033[36mv%s\033[0m\n" "$(VERSION)"
	@printf "\n\033[1mExamples:\033[0m\n"
	@printf "  \033[36mmake build\033[0m\n"
	@printf "  \033[36mmake check\033[0m\n"
	@printf "  \033[36mmake dist OS=darwin ARCH=arm64\033[0m\n"
	@printf "  \033[36mmake dist OS=darwin,linux ARCH=amd64,arm64\033[0m\n"
	@printf "  \033[36mmake -n release\033[0m\n"
	@printf "  \033[36mmake release\033[0m\n"
