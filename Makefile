# From-source entry points. Run from the repository root.
#   pnpm build / make build     engine, CLI, and workers
#   pnpm test  / make test      cargo test --workspace (builds engine bins first)
#   pnpm check / make check     fmt, clippy, and tests
#   pnpm desktop                stub engine/workers, then llama.cpp LLM worker, then Tauri/Vue
#   pnpm llama                  overwrite aifs-worker-llm with llama.cpp (needs a C++ compiler)
#   pnpm llama:cuda             same with CUDA (pins GPU SM; cargo looks idle on llama-cpp-sys-2)
#   pnpm cli -- scan fixtures/inbox-mixed

.PHONY: build test desktop-test check fmt clippy desktop cli llama llama-cuda help

help:
	@printf '%s\n' \
		'pnpm build     cargo engine-bins (engine, CLI, workers)' \
		'pnpm test      cargo test --workspace' \
		'pnpm check     fmt + clippy + test' \
		'pnpm desktop   build binaries, llama worker, and start Tauri/Vue' \
		'pnpm cli -- scan fixtures/inbox-mixed' \
		'pnpm llama     llama.cpp LLM worker (needs a C++ compiler; used by pnpm desktop; wraps CMAKE_GENERATOR on Windows)' \
		'pnpm llama:cuda llama.cpp LLM worker with CUDA (pins GPU SM; cargo looks idle while nvcc runs)' \
		'make …         same targets without going through pnpm (cargo still runs)'

build:
	cargo engine-bins

test: build
	cargo test --workspace

desktop-test:
	pnpm install
	pnpm test:desktop

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets

check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets
	$(MAKE) test

desktop: build
	$(MAKE) llama
	pnpm install
	pnpm --filter desktop tauri dev

cli: build
	./target/debug/aifs $(ARGS)

llama:
ifeq ($(OS),Windows_NT)
	node scripts/with-cmake-generator.mjs cargo engine-llm
else
	CXX=$${CXX:-g++} cargo engine-llm
endif

llama-cuda:
	node scripts/with-cmake-generator.mjs cargo engine-llm --features llama,cuda
