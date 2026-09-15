# From-source entry points. Run from the repository root.
#   pnpm build / make build     engine, CLI, and workers
#   pnpm test  / make test      cargo test --workspace (builds engine bins first)
#   pnpm check / make check     fmt, clippy, and tests
#   pnpm desktop                engine/workers, then Tauri (reuses staged llm-runtime; llama compiles if missing)
#   pnpm desktop:cuda           same with CUDA (sets AIFS_LLM_FEATURES so Tauri does not rebuild CPU llama)
#   pnpm desktop:open:cuda      start Tauri only; skip llama when llm-runtime/cuda is already staged
#   pnpm desktop:vulkan         same with Vulkan
#   pnpm llama                  llama.cpp LLM worker; stages llm-runtime/<accel>/ (needs a C++ compiler)
#   pnpm llama:cuda             same with CUDA (pins GPU SM; cargo looks idle on llama-cpp-sys-2)
#   pnpm llama:vulkan           same with Vulkan
#   pnpm cli -- scan fixtures/inbox-mixed

.PHONY: build test desktop-test check fmt clippy desktop desktop-cuda desktop-vulkan cli llama llama-cuda llama-vulkan help

help:
	@printf '%s\n' \
		'pnpm build     cargo engine-bins (engine, CLI, workers)' \
		'pnpm test      cargo test --workspace' \
		'pnpm check     fmt + clippy + test' \
		'pnpm desktop   start Tauri/Vue (reuses staged llm-runtime; compiles llama if missing)' \
		'pnpm desktop:cuda  same with CUDA (AIFS_LLM_FEATURES; skips llama when cuda payload exists)' \
		'pnpm desktop:open:cuda  start Tauri only (no pnpm build; still skips llama if staged)' \
		'pnpm desktop:vulkan same with Vulkan' \
		'pnpm cli -- scan fixtures/inbox-mixed' \
		'pnpm llama     llama.cpp LLM worker (needs a C++ compiler; used by pnpm desktop; wraps CMAKE_GENERATOR on Windows)' \
		'pnpm llama:cuda llama.cpp LLM worker with CUDA (pins GPU SM; cargo looks idle while nvcc runs)' \
		'pnpm llama:vulkan llama.cpp LLM worker with Vulkan' \
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
	pnpm install
	pnpm --filter desktop tauri dev

cli: build
	./target/debug/aifs $(ARGS)

llama:
	node scripts/with-cmake-generator.mjs cargo engine-llm

llama-cuda:
	node scripts/with-llm-features.mjs cuda node scripts/with-cmake-generator.mjs cargo engine-llm

llama-vulkan:
	node scripts/with-llm-features.mjs vulkan node scripts/with-cmake-generator.mjs cargo engine-llm

desktop-cuda:
	node scripts/with-llm-features.mjs cuda $(MAKE) desktop

desktop-vulkan:
	node scripts/with-llm-features.mjs vulkan $(MAKE) desktop
