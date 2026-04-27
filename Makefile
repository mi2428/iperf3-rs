SHELL         := /bin/bash
.SHELLFLAGS   := -eu -o pipefail -c
.DEFAULT_GOAL := help

RUSTUP           ?= rustup
RUSTUP_TOOLCHAIN ?= 1.93.0
CARGO            ?= $(shell if command -v $(RUSTUP) >/dev/null 2>&1 && $(RUSTUP) which cargo --toolchain $(RUSTUP_TOOLCHAIN) >/dev/null 2>&1; then $(RUSTUP) which cargo --toolchain $(RUSTUP_TOOLCHAIN); else command -v cargo; fi)
RUSTC            ?= $(shell if command -v $(RUSTUP) >/dev/null 2>&1 && $(RUSTUP) which rustc --toolchain $(RUSTUP_TOOLCHAIN) >/dev/null 2>&1; then $(RUSTUP) which rustc --toolchain $(RUSTUP_TOOLCHAIN); else command -v rustc; fi)
RUSTDOC          ?= $(shell if command -v $(RUSTUP) >/dev/null 2>&1 && $(RUSTUP) which rustdoc --toolchain $(RUSTUP_TOOLCHAIN) >/dev/null 2>&1; then $(RUSTUP) which rustdoc --toolchain $(RUSTUP_TOOLCHAIN); else command -v rustdoc; fi)
RUST_BINDIR      := $(patsubst %/,%,$(dir $(CARGO)))
CARGO_ENV        := PATH="$(RUST_BINDIR):$(PATH)" RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)"

INSTALL ?= install
DOCKER  ?= docker
COMPOSE ?= $(shell if $(DOCKER) compose version >/dev/null 2>&1; then printf '%s compose' '$(DOCKER)'; elif command -v docker-compose >/dev/null 2>&1; then command -v docker-compose; else printf '%s compose' '$(DOCKER)'; fi)
MULTIPASS ?= multipass
GIT_REMOTE ?= origin

APP            := iperf3-rs
BINDIR         := bin
COMPLETION_DIR := completions
DISTDIR        := dist
TEST_COMPOSE   := docker-compose.test.yml
EXAMPLES       ?=
NO_DEFAULT     ?=
TEST_FEATURE_FLAGS := $(if $(NO_DEFAULT),--no-default-features)

INSTALL_PREFIX      ?= $(HOME)/.local
INSTALL_BINDIR      ?= $(INSTALL_PREFIX)/bin
BASH_COMPLETION_DIR ?= $(INSTALL_PREFIX)/share/bash-completion/completions
DETECTED_ZSH_COMPLETION_DIR := $(shell \
	if command -v zsh >/dev/null 2>&1; then \
		zsh -fc 'print -rl -- $${fpath[@]}' 2>/dev/null | \
			while IFS= read -r dir; do \
				if [ "$${dir%/site-functions}" != "$$dir" ] && [ -d "$$dir" ] && [ -w "$$dir" ]; then \
					printf '%s\n' "$$dir"; \
					exit 0; \
				fi; \
			done; \
	fi)
ZSH_COMPLETION_DIR  ?= $(or $(DETECTED_ZSH_COMPLETION_DIR),$(INSTALL_PREFIX)/share/zsh/site-functions)
FISH_COMPLETION_DIR ?= $(INSTALL_PREFIX)/share/fish/vendor_completions.d
OS                  ?= darwin,linux
ARCH                ?= amd64,arm64

DARWIN_ARCHS := amd64 arm64
LINUX_ARCHS  := amd64 arm64
RUST_TARGETS := x86_64-apple-darwin aarch64-apple-darwin

DARWIN_amd64_TARGET := x86_64-apple-darwin
DARWIN_amd64_SUFFIX := darwin-amd64
DARWIN_arm64_TARGET := aarch64-apple-darwin
DARWIN_arm64_SUFFIX := darwin-arm64

LINUX_amd64_PLATFORM := linux/amd64
LINUX_amd64_SUFFIX   := linux-amd64
LINUX_arm64_PLATFORM := linux/arm64
LINUX_arm64_SUFFIX   := linux-arm64
LINUX_BUILD_IMAGE    ?= rust:1.93-bullseye
LINUX_SMOKE_IMAGE    ?= debian:bullseye-slim
LINUX_CACHE_KEY      := $(shell printf '%s' '$(LINUX_BUILD_IMAGE)' | sed 's/[^A-Za-z0-9_.-]/-/g')
DOCKER_UID           ?= $(shell id -u)
DOCKER_GID           ?= $(shell id -g)
HOST_OS              := $(shell uname -s)

MULTIPASS_NAME       ?= iperf3-rs-dev
MULTIPASS_IMAGE      ?= 24.04
MULTIPASS_CPUS       ?= 2
MULTIPASS_MEMORY     ?= 4G
MULTIPASS_DISK       ?= 20G
MULTIPASS_SOURCE_DIR ?= iperf3-rs

##@ Development

.PHONY: build
build: ## Build the host binary into bin/
	@mkdir -p $(BINDIR)
	@$(CARGO_ENV) $(CARGO) build --release
	@cp target/release/$(APP) $(BINDIR)/$(APP)
	@chmod +x $(BINDIR)/$(APP)
	@printf 'Wrote %s/%s\n' "$(BINDIR)" "$(APP)"

.PHONY: install
install: ## Build and install the host binary into INSTALL_BINDIR
	@$(CARGO_ENV) $(CARGO) build --release
	@mkdir -p "$(INSTALL_BINDIR)"
	@$(INSTALL) -m 0755 "target/release/$(APP)" "$(INSTALL_BINDIR)/$(APP)"
	@printf 'Installed %s\n' "$(INSTALL_BINDIR)/$(APP)"
	@if [ "$(COMPLETION)" = "1" ]; then \
		$(MAKE) --no-print-directory _completions MODE=install; \
	fi

.PHONY: _completions
_completions:
	@mode="$(MODE)"; \
	if [ "$(CHECK_ONLY)" = "1" ]; then \
		mode="check"; \
	fi; \
	case "$$mode" in \
		""|check) \
			bash -n "$(COMPLETION_DIR)/$(APP).bash"; \
			if command -v zsh >/dev/null 2>&1; then \
				zsh -n "$(COMPLETION_DIR)/_$(APP)"; \
			else \
				printf 'Skipping zsh completion check; zsh not found\n'; \
			fi; \
			if command -v fish >/dev/null 2>&1; then \
				fish -n "$(COMPLETION_DIR)/$(APP).fish"; \
			else \
				printf 'Skipping fish completion check; fish not found\n'; \
			fi; \
			;; \
		install) \
			mkdir -p "$(BASH_COMPLETION_DIR)" "$(ZSH_COMPLETION_DIR)" "$(FISH_COMPLETION_DIR)"; \
			$(INSTALL) -m 0644 "$(COMPLETION_DIR)/$(APP).bash" "$(BASH_COMPLETION_DIR)/$(APP)"; \
			$(INSTALL) -m 0644 "$(COMPLETION_DIR)/_$(APP)" "$(ZSH_COMPLETION_DIR)/_$(APP)"; \
			$(INSTALL) -m 0644 "$(COMPLETION_DIR)/$(APP).fish" "$(FISH_COMPLETION_DIR)/$(APP).fish"; \
			printf 'Installed bash completion to %s/%s\n' "$(BASH_COMPLETION_DIR)" "$(APP)"; \
			printf 'Installed zsh completion to %s/_%s\n' "$(ZSH_COMPLETION_DIR)" "$(APP)"; \
			printf 'Installed fish completion to %s/%s.fish\n' "$(FISH_COMPLETION_DIR)" "$(APP)"; \
			if command -v zsh >/dev/null 2>&1 && ! zsh -fc 'target=$$1; for dir in $${fpath[@]}; do [[ "$$dir" == "$$target" ]] && exit 0; done; exit 1' -- "$(ZSH_COMPLETION_DIR)"; then \
				printf 'Note: zsh completion dir is not in fpath; add before compinit: fpath=(%s $$fpath)\n' "$(ZSH_COMPLETION_DIR)"; \
			fi; \
			;; \
		*) \
			echo "Unsupported MODE '$$mode'. Supported values: check, install" >&2; \
			exit 1; \
			;; \
	esac

.PHONY: fmt
fmt: ## Format Rust sources. Use CHECK_ONLY=1 to check without writing
	@if [ "$(CHECK_ONLY)" = "1" ]; then \
		$(CARGO_ENV) $(CARGO) fmt --all --check; \
	else \
		$(CARGO_ENV) $(CARGO) fmt --all; \
	fi

.PHONY: lint
lint: ## Run clippy with warnings treated as errors
	@$(CARGO_ENV) $(CARGO) clippy --all-targets --all-features -- -D warnings

.PHONY: doc
doc: ## Build rustdoc with warnings treated as errors
	@RUSTDOCFLAGS="-D warnings" $(CARGO_ENV) $(CARGO) doc --no-deps

.PHONY: test
test: ## Run unit tests. Use NO_DEFAULT=1 to disable default features
	@$(CARGO_ENV) $(CARGO) test $(TEST_FEATURE_FLAGS)

.PHONY: kani
kani: ## Run Kani model checking harnesses
	@$(CARGO_ENV) $(CARGO) kani

.PHONY: e2e
e2e: ## Run Docker E2E tests
	@COMPOSE="$(COMPOSE)" DOCKER="$(DOCKER)" $(CARGO_ENV) $(CARGO) test --test e2e_test -- --ignored --nocapture --test-threads=1

.PHONY: integration
integration: ## Run local integration tests. Use EXAMPLES=name,all for examples
	@examples="$(EXAMPLES)"; \
	if [ -z "$$examples" ]; then \
		$(CARGO_ENV) $(CARGO) test --test integration_test; \
		exit 0; \
	fi; \
	if [ "$$examples" = "all" ]; then \
		examples="$$(for compose in examples/*/docker-compose.test.yml; do \
			[ -f "$$compose" ] || continue; \
			basename "$$(dirname "$$compose")"; \
		done | sort | tr '\n' ' ')"; \
	else \
		examples="$$(printf '%s' "$$examples" | tr ',' ' ')"; \
	fi; \
	if [ -z "$$examples" ]; then \
		echo "No example integration tests found" >&2; \
		exit 1; \
	fi; \
	for example in $$examples; do \
		manifest="examples/$$example/Cargo.toml"; \
		test_file="examples/$$example/integration_test.rs"; \
		if [ ! -f "$$manifest" ]; then \
			echo "Example '$$example' has no Cargo.toml" >&2; \
			exit 1; \
		fi; \
		if [ ! -f "$$test_file" ]; then \
			echo "Example '$$example' has no integration_test.rs" >&2; \
			exit 1; \
		fi; \
		printf 'Running example integration %s\n' "$$example"; \
		COMPOSE="$(COMPOSE)" DOCKER="$(DOCKER)" CARGO_TARGET_DIR="$(CURDIR)/target/examples/$$example" $(CARGO_ENV) $(CARGO) test --manifest-path "$$manifest" --test integration_test -- --ignored --nocapture --test-threads=1; \
	done

.PHONY: check
check: ## Run formatting, lint, tests, and completion checks
	@$(MAKE) --no-print-directory fmt CHECK_ONLY=1
	@$(MAKE) --no-print-directory lint
	@$(MAKE) --no-print-directory doc
	@$(MAKE) --no-print-directory test
	@$(MAKE) --no-print-directory test NO_DEFAULT=1
	@$(MAKE) --no-print-directory _completions CHECK_ONLY=1

.PHONY: multipass
multipass: ## Launch a Multipass VM and copy the source tree for manual Linux testing
	@command -v $(MULTIPASS) >/dev/null 2>&1 || { \
		echo "Multipass is required for this target" >&2; \
		exit 1; \
	}
	@if $(MULTIPASS) info "$(MULTIPASS_NAME)" >/dev/null 2>&1; then \
		printf 'Starting existing Multipass VM %s\n' "$(MULTIPASS_NAME)"; \
		$(MULTIPASS) start "$(MULTIPASS_NAME)" >/dev/null; \
	else \
		printf 'Launching Multipass VM %s from %s\n' "$(MULTIPASS_NAME)" "$(MULTIPASS_IMAGE)"; \
		$(MULTIPASS) launch "$(MULTIPASS_IMAGE)" \
			--name "$(MULTIPASS_NAME)" \
			--cpus "$(MULTIPASS_CPUS)" \
			--memory "$(MULTIPASS_MEMORY)" \
			--disk "$(MULTIPASS_DISK)"; \
	fi
	@archive="$$(mktemp "$${TMPDIR:-/tmp}/$(APP)-multipass.XXXXXX.tar.gz")"; \
	trap 'rm -f "$$archive"' EXIT; \
	printf 'Packing source tree for %s\n' "$(MULTIPASS_NAME)"; \
	tar_metadata_flags=(); \
	for flag in --no-xattrs --no-mac-metadata --disable-copyfile; do \
		if tar "$$flag" -cf /dev/null --files-from /dev/null >/dev/null 2>&1; then \
			tar_metadata_flags+=("$$flag"); \
		fi; \
	done; \
	COPYFILE_DISABLE=1 tar "$${tar_metadata_flags[@]}" \
		--exclude './target' \
		--exclude './dist' \
		--exclude './bin' \
		--exclude './.cargo-linux' \
		--exclude './.home-linux' \
		--exclude './.DS_Store' \
		-czf "$$archive" .; \
	$(MULTIPASS) exec "$(MULTIPASS_NAME)" -- rm -rf "/home/ubuntu/$(MULTIPASS_SOURCE_DIR)"; \
	$(MULTIPASS) exec "$(MULTIPASS_NAME)" -- mkdir -p "/home/ubuntu/$(MULTIPASS_SOURCE_DIR)"; \
	$(MULTIPASS) transfer "$$archive" "$(MULTIPASS_NAME):/tmp/$(APP)-source.tar.gz"; \
	$(MULTIPASS) exec "$(MULTIPASS_NAME)" -- tar -xzf "/tmp/$(APP)-source.tar.gz" -C "/home/ubuntu/$(MULTIPASS_SOURCE_DIR)"; \
	$(MULTIPASS) exec "$(MULTIPASS_NAME)" -- chown -R ubuntu:ubuntu "/home/ubuntu/$(MULTIPASS_SOURCE_DIR)"; \
	printf '\nMultipass VM is ready.\n'; \
	printf '\nRun these commands to build inside the VM:\n'; \
	printf '  1) %s\n' "$(MULTIPASS) shell $(MULTIPASS_NAME)"; \
	printf '  2) %s\n' "cd ~/$(MULTIPASS_SOURCE_DIR)"; \
	printf '  3) %s\n' "sudo apt-get update && sudo apt-get install -y --no-install-recommends build-essential ca-certificates curl make pkg-config"; \
	printf '  4) %s\n' "command -v cargo >/dev/null || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"; \
	printf '  5) %s\n' "source \"\$$HOME/.cargo/env\""; \
	printf '  6) %s\n' "make build"; \
	printf '  7) %s\n' "./bin/iperf3-rs --version"

.PHONY: clean
clean: ## Remove local build artifacts
	@rm -rf $(BINDIR) $(DISTDIR) .cargo-linux .home-linux
	@$(CARGO_ENV) $(CARGO) clean

##@ Distribution

.PHONY: release
release: ## Tag, push, and publish the crate to crates.io. Requires TAG=vX.Y.Z
	@TAG="$(TAG)" GIT_REMOTE="$(GIT_REMOTE)" CARGO="$(CARGO)" RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)" PATH="$(RUST_BINDIR):$(PATH)" bash scripts/release.sh

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
	$(MAKE) dist-smoke; \
	$(MAKE) checksums

.PHONY: dist-smoke
dist-smoke: ## Smoke-test Linux dist binaries in an old-glibc Debian container
	@if ! ls "$(DISTDIR)"/$(APP)-linux-* >/dev/null 2>&1; then \
		printf 'Skipping Linux dist smoke test; no Linux artifacts found\n'; \
		exit 0; \
	fi
	@$(MAKE) --no-print-directory _docker-check
	@for arch in $(LINUX_ARCHS); do \
		case "$$arch" in \
			amd64) binary="$(DISTDIR)/$(APP)-$(LINUX_amd64_SUFFIX)"; platform="$(LINUX_amd64_PLATFORM)" ;; \
			arm64) binary="$(DISTDIR)/$(APP)-$(LINUX_arm64_SUFFIX)"; platform="$(LINUX_arm64_PLATFORM)" ;; \
			*) echo "Unsupported Linux ARCH '$$arch'" >&2; exit 1 ;; \
		esac; \
		if [ ! -f "$$binary" ]; then \
			continue; \
		fi; \
		printf 'Smoke-testing %s on %s in %s\n' "$$binary" "$$platform" "$(LINUX_SMOKE_IMAGE)"; \
		$(DOCKER) run --rm \
			--platform "$$platform" \
			-v "$(CURDIR):/workspace:ro" \
			-w /workspace \
			$(LINUX_SMOKE_IMAGE) \
			"/workspace/$$binary" -h >/dev/null; \
		$(DOCKER) run --rm \
			--platform "$$platform" \
			-v "$(CURDIR):/workspace:ro" \
			-w /workspace \
			$(LINUX_SMOKE_IMAGE) \
			"/workspace/$$binary" --version >/dev/null; \
	done

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
	@$(CARGO_ENV) $(CARGO) build --release --target $$(DARWIN_$(1)_TARGET)
	@cp target/$$(DARWIN_$(1)_TARGET)/release/$(APP) $(DISTDIR)/$(APP)-$$(DARWIN_$(1)_SUFFIX)
	@chmod +x $(DISTDIR)/$(APP)-$$(DARWIN_$(1)_SUFFIX)
	@printf 'Wrote %s/%s-%s\n' "$(DISTDIR)" "$(APP)" "$$(DARWIN_$(1)_SUFFIX)"
endef
$(foreach arch,$(DARWIN_ARCHS),$(eval $(call DARWIN_DIST_RULE,$(arch))))

define LINUX_DIST_RULE
.PHONY: _dist.linux.$(1)
_dist.linux.$(1): _docker-check
	@printf 'Building %s for %s via Docker\n' "$(APP)" "$$(LINUX_$(1)_PLATFORM)"
	@mkdir -p $(DISTDIR) .cargo-linux/$(1) .home-linux/$(LINUX_CACHE_KEY)/$(1)
	@$(DOCKER) run --rm \
		--platform $$(LINUX_$(1)_PLATFORM) \
		-e HOME=/workspace/.home-linux/$(LINUX_CACHE_KEY)/$(1) \
		-e CARGO_HOME=/workspace/.cargo-linux/$(1) \
		-e CARGO_TARGET_DIR=/workspace/target/linux-$(1)-$(LINUX_CACHE_KEY) \
		-v "$(CURDIR):/workspace" \
		-w /workspace \
		$(LINUX_BUILD_IMAGE) \
		bash -eu -o pipefail -c ' \
			apt-get update >/dev/null; \
			apt-get install -y --no-install-recommends build-essential make pkg-config ca-certificates >/dev/null; \
			cargo build --release; \
			cp target/linux-$(1)-$(LINUX_CACHE_KEY)/release/$(APP) dist/$(APP)-$$(LINUX_$(1)_SUFFIX); \
			chmod +x dist/$(APP)-$$(LINUX_$(1)_SUFFIX); \
			chown -R $(DOCKER_UID):$(DOCKER_GID) dist target/linux-$(1)-$(LINUX_CACHE_KEY) .cargo-linux/$(1) .home-linux/$(LINUX_CACHE_KEY)/$(1)'
	@printf 'Wrote %s/%s-%s\n' "$(DISTDIR)" "$(APP)" "$$(LINUX_$(1)_SUFFIX)"
endef
$(foreach arch,$(LINUX_ARCHS),$(eval $(call LINUX_DIST_RULE,$(arch))))

##@ Help

.PHONY: help
help: ## Show this help message
	@awk 'BEGIN {FS = ":.*##"; width = 0} \
		{ lines[NR] = $$0 } \
		/^[a-zA-Z0-9_.-]+:.*##/ { if (length($$1) > width) width = length($$1) } \
		END { \
			section = ""; \
			width += 2; \
			for (i = 1; i <= NR; i++) { \
				$$0 = lines[i]; \
				if ($$0 ~ /^##@/) { \
					section = substr($$0, 5); \
				} else if ($$0 ~ /^[a-zA-Z0-9_.-]+:.*##/) { \
					split($$0, parts, ":.*##"); \
					if (section != "") printf "\n\033[1m%s\033[0m\n", section; \
					section = ""; \
					printf "  \033[36m%-*s\033[0m %s\n", width, parts[1], parts[2]; \
				} \
			} \
		}' $(MAKEFILE_LIST)
	@printf "\n\033[1mVariables:\033[0m\n"
	@printf "  \033[36mTAG\033[0m                    Release tag for \033[36mmake release\033[0m, for example \033[36mv1.0.0\033[0m\n"
	@printf "  \033[36mGIT_REMOTE\033[0m             Release git remote, defaults to \033[36m%s\033[0m\n" "$(GIT_REMOTE)"
	@printf "  \033[36mOS\033[0m                     Release OS list: \033[36mdarwin,linux\033[0m\n"
	@printf "  \033[36mARCH\033[0m                   Release arch list: \033[36mamd64,arm64\033[0m\n"
	@printf "  \033[36mEXAMPLES\033[0m               Example integration tests for \033[36mmake integration\033[0m: \033[36mbwcheck,all\033[0m\n"
	@printf "  \033[36mNO_DEFAULT\033[0m             Disable default Cargo features for \033[36mmake test\033[0m when set, for example \033[36m1\033[0m\n"
	@printf "  \033[36mINSTALL_BINDIR\033[0m         Install directory, defaults to \033[36m%s\033[0m\n" "$(INSTALL_BINDIR)"
	@printf "  \033[36mBASH_COMPLETION_DIR\033[0m    Bash completion install dir, defaults to \033[36m%s\033[0m\n" "$(BASH_COMPLETION_DIR)"
	@printf "  \033[36mZSH_COMPLETION_DIR\033[0m     Zsh completion install dir, defaults to \033[36m%s\033[0m\n" "$(ZSH_COMPLETION_DIR)"
	@printf "  \033[36mFISH_COMPLETION_DIR\033[0m    Fish completion install dir, defaults to \033[36m%s\033[0m\n" "$(FISH_COMPLETION_DIR)"
	@printf "  \033[36mMULTIPASS_NAME\033[0m         Multipass VM name, defaults to \033[36m%s\033[0m\n" "$(MULTIPASS_NAME)"
	@printf "\n\033[1mExamples:\033[0m\n"
	@printf "  \033[36m%-44s\033[0m # to check formatting without writing\n" "make fmt CHECK_ONLY=1"
	@printf "  \033[36m%-44s\033[0m # to run tests without default features\n" "make test NO_DEFAULT=1"
	@printf "  \033[36m%-44s\033[0m # to build and install the host binary and completions\n" "make install COMPLETION=1"
	@printf "  \033[36m%-44s\033[0m # to run Docker E2E tests\n" "make e2e"
	@printf "  \033[36m%-44s\033[0m # to run local integration tests\n" "make integration"
	@printf "  \033[36m%-44s\033[0m # to run a specific example integration test\n" "make integration EXAMPLES=bwcheck"
	@printf "  \033[36m%-44s\033[0m # to run all release-blocking quality gates\n" "make check e2e kani"
	@printf "  \033[36m%-44s\033[0m # to publish crates.io and push the release tag\n" "make release TAG=v1.0.0"
	@printf "  \033[36m%-44s\033[0m # to build release binaries and checksums\n" "make dist OS=darwin,linux ARCH=amd64,arm64"
	@printf "  \033[36m%-44s\033[0m # to prepare a Linux VM for manual testing\n" "make multipass"
