# From-source entry points. Run from the repository root.
#   make build          engine, CLI, and workers
#   make test           cargo test --workspace (builds engine bins first)
#   make check          fmt, clippy, and tests
#   make desktop        build binaries, then Tauri/Vue dev
#   make cli ARGS='scan fixtures/inbox-mixed'

ENGINE_PACKAGES = aifs-engine aifs-cli aifs-worker-media aifs-worker-document aifs-worker-vision aifs-worker-llm
ENGINE_PACKAGE_FLAGS = $(foreach pkg,$(ENGINE_PACKAGES),-p $(pkg))
DESKTOP_DIR = apps/desktop

.PHONY: build test desktop-test check fmt clippy desktop cli llama help

help:
	@printf '%s\n' \
		'make build     cargo build (engine, CLI, workers)' \
		'make test      cargo test --workspace' \
		'make check     fmt + clippy + test' \
		'make desktop   build binaries and start Tauri/Vue' \
		"make cli ARGS='scan fixtures/inbox-mixed'" \
		'make llama     optional llama.cpp LLM worker (needs a C++ compiler)'

build:
	cargo build $(ENGINE_PACKAGE_FLAGS)

test: build
	cargo test --workspace

desktop-test:
	cd $(DESKTOP_DIR) && pnpm install && pnpm test && pnpm exec vue-tsc --noEmit

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets

check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets
	$(MAKE) test

desktop: build
	cd $(DESKTOP_DIR) && pnpm install && pnpm tauri dev

cli: build
	./target/debug/aifs $(ARGS)

llama:
	CXX=$${CXX:-g++} cargo build -p aifs-worker-llm --features llama
