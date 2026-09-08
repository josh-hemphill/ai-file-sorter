# From-source entry points. Run from the repository root.
#   pnpm build / make build     engine, CLI, and workers
#   pnpm test  / make test      cargo test --workspace (builds engine bins first)
#   pnpm check / make check     fmt, clippy, and tests
#   pnpm desktop                stub engine/workers, then llama.cpp LLM worker, then Tauri/Vue
#   pnpm llama                  overwrite aifs-worker-llm with llama.cpp (needs a C++ compiler)
#   pnpm cli -- scan fixtures/inbox-mixed

.PHONY: build test desktop-test check fmt clippy desktop cli llama help

help:
	@printf '%s\n' \
		'pnpm build     cargo engine-bins (engine, CLI, workers)' \
		'pnpm test      cargo test --workspace' \
		'pnpm check     fmt + clippy + test' \
		'pnpm desktop   build binaries, llama worker, and start Tauri/Vue' \
		'pnpm cli -- scan fixtures/inbox-mixed' \
		'pnpm llama     llama.cpp LLM worker (needs a C++ compiler; used by pnpm desktop)' \
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
	CXX=$${CXX:-g++} cargo engine-llm
