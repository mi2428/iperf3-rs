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
GIT     ?= git
GH      ?= gh
DOCKER  ?= docker
COMPOSE ?= $(shell if $(DOCKER) compose version >/dev/null 2>&1; then printf '%s compose' '$(DOCKER)'; elif command -v docker-compose >/dev/null 2>&1; then command -v docker-compose; else printf '%s compose' '$(DOCKER)'; fi)

REMOTE            ?= origin
MAIN_BRANCH       ?= main
GITHUB_REPOSITORY ?= $(shell $(GIT) remote get-url $(REMOTE) 2>/dev/null | awk '{ gsub(/^git@github.com:/, ""); gsub(/^ssh:\/\/git@github.com\//, ""); gsub(/^https:\/\/github.com\//, ""); gsub(/\.git$$/, ""); print tolower($$0) }')
GIT_DESCRIBE      ?= $(shell $(GIT) describe --tags --always --dirty=-dirty 2>/dev/null || printf 'unknown')
GIT_COMMIT        ?= $(shell $(GIT) rev-parse HEAD 2>/dev/null || printf 'unknown')
GIT_COMMIT_DATE   ?= $(shell $(GIT) show -s --format=%cI HEAD 2>/dev/null || printf 'unknown')
BUILD_DATE        ?= $(shell date -u +%Y-%m-%dT%H:%M:%SZ)
DOCKER_BUILD_METADATA_ARGS := --build-arg IPERF3_RS_BUILD_DATE="$(BUILD_DATE)" --build-arg IPERF3_RS_GIT_COMMIT="$(GIT_COMMIT)" --build-arg IPERF3_RS_GIT_COMMIT_DATE="$(GIT_COMMIT_DATE)" --build-arg IPERF3_RS_GIT_DESCRIBE="$(GIT_DESCRIBE)"

APP            := iperf3-rs
BINDIR         := bin
COMPLETION_DIR := completions
DISTDIR        := dist
TEST_COMPOSE   := docker-compose.test.yml
VERSION        := $(shell awk 'BEGIN { in_pkg = 0 } /^\[package\]$$/ { in_pkg = 1; next } /^\[/ { in_pkg = 0 } in_pkg && $$1 == "version" { gsub(/"/, "", $$3); print $$3; exit }' Cargo.toml)

INSTALL_PREFIX      ?= $(HOME)/.local
INSTALL_BINDIR      ?= $(INSTALL_PREFIX)/bin
BASH_COMPLETION_DIR ?= $(INSTALL_PREFIX)/share/bash-completion/completions
ZSH_COMPLETION_DIR  ?= $(INSTALL_PREFIX)/share/zsh/site-functions
FISH_COMPLETION_DIR ?= $(INSTALL_PREFIX)/share/fish/vendor_completions.d
TAG                 ?= v$(VERSION)
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
MAIN_REMOTE_REF        := refs/remotes/$(REMOTE)/$(MAIN_BRANCH)
GHCR_REGISTRY          ?= ghcr.io
GHCR_REPOSITORY        ?= $(GITHUB_REPOSITORY)
GHCR_IMAGE             ?= $(GHCR_REGISTRY)/$(GHCR_REPOSITORY)
GHCR_USER              ?= $(firstword $(subst /, ,$(GHCR_REPOSITORY)))
GHCR_PLATFORMS         ?= linux/amd64,linux/arm64
GHCR_TAGS              ?= $(TAG)
GHCR_TARGET            ?= release
GHCR_LOGIN             ?= true

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

.PHONY: install-completions
install-completions: ## Install bash, zsh, and fish completion files
	@mkdir -p "$(BASH_COMPLETION_DIR)" "$(ZSH_COMPLETION_DIR)" "$(FISH_COMPLETION_DIR)"
	@$(INSTALL) -m 0644 "$(COMPLETION_DIR)/$(APP).bash" "$(BASH_COMPLETION_DIR)/$(APP)"
	@$(INSTALL) -m 0644 "$(COMPLETION_DIR)/_$(APP)" "$(ZSH_COMPLETION_DIR)/_$(APP)"
	@$(INSTALL) -m 0644 "$(COMPLETION_DIR)/$(APP).fish" "$(FISH_COMPLETION_DIR)/$(APP).fish"
	@printf 'Installed bash completion to %s/%s\n' "$(BASH_COMPLETION_DIR)" "$(APP)"
	@printf 'Installed zsh completion to %s/_%s\n' "$(ZSH_COMPLETION_DIR)" "$(APP)"
	@printf 'Installed fish completion to %s/%s.fish\n' "$(FISH_COMPLETION_DIR)" "$(APP)"

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

.PHONY: completion-check
completion-check: ## Syntax-check shell completion files when shells are available
	@bash -n "$(COMPLETION_DIR)/$(APP).bash"
	@if command -v zsh >/dev/null 2>&1; then \
		zsh -n "$(COMPLETION_DIR)/_$(APP)"; \
	else \
		printf 'Skipping zsh completion check; zsh not found\n'; \
	fi
	@if command -v fish >/dev/null 2>&1; then \
		fish -n "$(COMPLETION_DIR)/$(APP).fish"; \
	else \
		printf 'Skipping fish completion check; fish not found\n'; \
	fi

.PHONY: kani
kani: ## Run Kani model checking harnesses
	@$(CARGO_ENV) $(CARGO) kani

.PHONY: integration-test
integration-test: ## Run Docker Compose integration tests
	@COMPOSE="$(COMPOSE)" DOCKER="$(DOCKER)" $(CARGO_ENV) $(CARGO) test --test integration_test -- --ignored --nocapture --test-threads=1

.PHONY: check
check: fmt-check lint test completion-check ## Run formatting, lint, tests, and completion checks

.PHONY: verify
verify: check kani integration-test ## Run all release-blocking quality gates

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

.PHONY: _docker-buildx-check
_docker-buildx-check: _docker-check
	@$(DOCKER) buildx version >/dev/null 2>&1 || { \
		echo "Docker buildx is required to publish multi-arch images" >&2; \
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

.PHONY: _release-check
_release-check:
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

.PHONY: _ghcr-login
_ghcr-login:
	@if [ "$(GHCR_LOGIN)" != "true" ]; then \
		exit 0; \
	fi; \
	command -v $(GH) >/dev/null 2>&1 || { \
		echo "gh is required to log in to GHCR" >&2; \
		exit 1; \
	}; \
	if [ -z "$(GHCR_USER)" ]; then \
		echo "GHCR_USER is required to log in to GHCR" >&2; \
		exit 1; \
	fi; \
	printf 'Logging in to %s as %s\n' "$(GHCR_REGISTRY)" "$(GHCR_USER)"; \
	$(GH) auth token | $(DOCKER) login "$(GHCR_REGISTRY)" -u "$(GHCR_USER)" --password-stdin >/dev/null

.PHONY: _publish-release-image
_publish-release-image: _docker-buildx-check _ghcr-login
	@if [ -z "$(TAG)" ]; then \
		echo "TAG is required for the release image publish step" >&2; \
		exit 1; \
	fi
	@if [ -z "$(TARGET_SHA)" ]; then \
		echo "TARGET_SHA is required for the release image publish step" >&2; \
		exit 1; \
	fi
	@if [ -z "$(GHCR_REPOSITORY)" ]; then \
		echo "GHCR_REPOSITORY is required for the release image publish step" >&2; \
		exit 1; \
	fi
	@if [ -z "$(GHCR_TAGS)" ]; then \
		echo "GHCR_TAGS is required for the release image publish step" >&2; \
		exit 1; \
	fi
	@repo="$(GHCR_REPOSITORY)"; \
	tag_args=(); \
	for image_tag in $$(printf '%s' "$(GHCR_TAGS)" | tr ',' ' '); do \
		tag_args+=(--tag "$(GHCR_IMAGE):$$image_tag"); \
	done; \
	if [ "$${#tag_args[@]}" -eq 0 ]; then \
		echo "GHCR_TAGS did not contain any image tags" >&2; \
		exit 1; \
	fi; \
		printf 'Publishing multi-arch image %s for %s on %s\n' "$(GHCR_IMAGE)" "$(TAG)" "$(GHCR_PLATFORMS)"; \
		$(DOCKER) buildx build \
			--platform "$(GHCR_PLATFORMS)" \
			--target "$(GHCR_TARGET)" \
			$(DOCKER_BUILD_METADATA_ARGS) \
			--push \
			--label "org.opencontainers.image.source=https://github.com/$$repo" \
		--label "org.opencontainers.image.revision=$(TARGET_SHA)" \
		--label "org.opencontainers.image.version=$(TAG)" \
		"$${tag_args[@]}" \
		.

.PHONY: _publish-release
_publish-release: _release-check
	@printf 'Creating GitHub release %s at %s\n' "$(TAG)" "$(TARGET_SHA)"
	@$(GH) release create "$(TAG)" "$(DISTDIR)"/$(APP)-* "$(DISTDIR)"/checksums.txt \
		--target "$(TARGET_SHA)" \
		--title "$(TAG)" \
		--notes "Release $(TAG) built from $(TARGET_SHA)"

.PHONY: release-image
release-image: ## Build and push the GHCR multi-arch release image for the current checkout
	@target_sha="$$($(GIT) rev-parse HEAD)"; \
	$(MAKE) _publish-release-image TAG="$(TAG)" TARGET_SHA="$$target_sha"

.PHONY: release
release: ## Build binaries for origin/main, publish a GitHub Release, and push the GHCR multi-arch image
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
	printf 'Running release quality gate for %s\n' "$$tag"; \
	"$$make_bin" -f "$(CURDIR)/Makefile" -C "$$tmpdir" verify; \
	printf 'Building release assets for %s\n' "$$tag"; \
	"$$make_bin" -f "$(CURDIR)/Makefile" -C "$$tmpdir" dist OS=darwin,linux ARCH=amd64,arm64; \
	printf 'Checking release state for %s\n' "$$tag"; \
	"$$make_bin" -f "$(CURDIR)/Makefile" -C "$$tmpdir" _release-check TAG="$$tag" TARGET_SHA="$$main_sha"; \
	printf 'Publishing GHCR image for %s\n' "$$tag"; \
	"$$make_bin" -f "$(CURDIR)/Makefile" -C "$$tmpdir" _publish-release-image TAG="$$tag" TARGET_SHA="$$main_sha"; \
	printf 'Publishing %s\n' "$$tag"; \
	"$$make_bin" -f "$(CURDIR)/Makefile" -C "$$tmpdir" _publish-release TAG="$$tag" TARGET_SHA="$$main_sha"

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
	@printf "  \033[36mTAG\033[0m                    GitHub release tag, defaults to \033[36mv%s\033[0m\n" "$(VERSION)"
	@printf "  \033[36mINSTALL_BINDIR\033[0m         Install directory, defaults to \033[36m%s\033[0m\n" "$(INSTALL_BINDIR)"
	@printf "  \033[36mBASH_COMPLETION_DIR\033[0m    Bash completion install dir, defaults to \033[36m%s\033[0m\n" "$(BASH_COMPLETION_DIR)"
	@printf "  \033[36mZSH_COMPLETION_DIR\033[0m     Zsh completion install dir, defaults to \033[36m%s\033[0m\n" "$(ZSH_COMPLETION_DIR)"
	@printf "  \033[36mFISH_COMPLETION_DIR\033[0m    Fish completion install dir, defaults to \033[36m%s\033[0m\n" "$(FISH_COMPLETION_DIR)"
	@printf "  \033[36mGHCR_IMAGE\033[0m             Release image, defaults to \033[36m%s\033[0m\n" "$(GHCR_IMAGE)"
	@printf "  \033[36mGHCR_PLATFORMS\033[0m         Release image platforms, defaults to \033[36m%s\033[0m\n" "$(GHCR_PLATFORMS)"
	@printf "  \033[36mGHCR_TAGS\033[0m              Release image tags, defaults to \033[36m%s\033[0m\n" "$(GHCR_TAGS)"
	@printf "  \033[36mGHCR_LOGIN\033[0m             Log in to GHCR with gh auth token before pushing, defaults to \033[36m%s\033[0m\n" "$(GHCR_LOGIN)"
	@printf "\n\033[1mExamples:\033[0m\n"
	@printf "  \033[36mmake build install\033[0m                          # to build and install the host binary\n"
	@printf "  \033[36mmake dist OS=darwin,linux ARCH=amd64,arm64\033[0m  # to build release binaries and checksums\n"
	@printf "  \033[36mmake release-image TAG=v0.1.0\033[0m               # to push the GHCR multi-arch image\n"
	@printf "  \033[36mmake -n release\033[0m                             # to preview release steps\n"
	@printf "  \033[36mmake release\033[0m                                # to verify, build, and publish a release\n"
