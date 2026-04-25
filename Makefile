SHELL         := /bin/bash
.SHELLFLAGS   := -eu -o pipefail -c
.DEFAULT_GOAL := help

RUSTUP           ?= rustup
RUSTUP_TOOLCHAIN ?= 1.95.0
CARGO            ?= $(shell if command -v $(RUSTUP) >/dev/null 2>&1 && $(RUSTUP) which cargo --toolchain $(RUSTUP_TOOLCHAIN) >/dev/null 2>&1; then $(RUSTUP) which cargo --toolchain $(RUSTUP_TOOLCHAIN); else command -v cargo; fi)
RUSTC            ?= $(shell if command -v $(RUSTUP) >/dev/null 2>&1 && $(RUSTUP) which rustc --toolchain $(RUSTUP_TOOLCHAIN) >/dev/null 2>&1; then $(RUSTUP) which rustc --toolchain $(RUSTUP_TOOLCHAIN); else command -v rustc; fi)
RUSTDOC          ?= $(shell if command -v $(RUSTUP) >/dev/null 2>&1 && $(RUSTUP) which rustdoc --toolchain $(RUSTUP_TOOLCHAIN) >/dev/null 2>&1; then $(RUSTUP) which rustdoc --toolchain $(RUSTUP_TOOLCHAIN); else command -v rustdoc; fi)
CARGO_ENV        := RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)"

INSTALL ?= install
DOCKER  ?= docker
COMPOSE ?= $(shell if $(DOCKER) compose version >/dev/null 2>&1; then printf '%s compose' '$(DOCKER)'; elif command -v docker-compose >/dev/null 2>&1; then command -v docker-compose; else printf '%s compose' '$(DOCKER)'; fi)

APP            := iperf3-rs
BINDIR         := bin
COMPLETION_DIR := completions
DISTDIR        := dist
TEST_COMPOSE   := docker-compose.test.yml

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
LINUX_BUILD_IMAGE    ?= rust:1.95-bookworm
DOCKER_UID           ?= $(shell id -u)
DOCKER_GID           ?= $(shell id -g)
HOST_OS              := $(shell uname -s)

RELEASE_CONFIGURE_ARGS ?= --without-openssl

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

.PHONY: test
test: ## Run unit tests
	@$(CARGO_ENV) $(CARGO) test

.PHONY: kani
kani: ## Run Kani model checking harnesses
	@$(CARGO_ENV) $(CARGO) kani

.PHONY: integration
integration: ## Run Docker Compose integration tests
	@COMPOSE="$(COMPOSE)" DOCKER="$(DOCKER)" $(CARGO_ENV) $(CARGO) test --test integration_test -- --ignored --nocapture --test-threads=1

.PHONY: check
check: ## Run formatting, lint, tests, and completion checks
	@$(MAKE) --no-print-directory fmt CHECK_ONLY=1
	@$(MAKE) --no-print-directory lint
	@$(MAKE) --no-print-directory test
	@$(MAKE) --no-print-directory _completions CHECK_ONLY=1

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
	@printf "  \033[36mOS\033[0m                     Release OS list: \033[36mdarwin,linux\033[0m\n"
	@printf "  \033[36mARCH\033[0m                   Release arch list: \033[36mamd64,arm64\033[0m\n"
	@printf "  \033[36mINSTALL_BINDIR\033[0m         Install directory, defaults to \033[36m%s\033[0m\n" "$(INSTALL_BINDIR)"
	@printf "  \033[36mBASH_COMPLETION_DIR\033[0m    Bash completion install dir, defaults to \033[36m%s\033[0m\n" "$(BASH_COMPLETION_DIR)"
	@printf "  \033[36mZSH_COMPLETION_DIR\033[0m     Zsh completion install dir, defaults to \033[36m%s\033[0m\n" "$(ZSH_COMPLETION_DIR)"
	@printf "  \033[36mFISH_COMPLETION_DIR\033[0m    Fish completion install dir, defaults to \033[36m%s\033[0m\n" "$(FISH_COMPLETION_DIR)"
	@printf "\n\033[1mExamples:\033[0m\n"
	@printf "  \033[36m%-44s\033[0m # to check formatting without writing\n" "make fmt CHECK_ONLY=1"
	@printf "  \033[36m%-44s\033[0m # to build and install the host binary and completions\n" "make install COMPLETION=1"
	@printf "  \033[36m%-44s\033[0m # to run all release-blocking quality gates\n" "make check integration kani"
	@printf "  \033[36m%-44s\033[0m # to build release binaries and checksums\n" "make dist OS=darwin,linux ARCH=amd64,arm64"
