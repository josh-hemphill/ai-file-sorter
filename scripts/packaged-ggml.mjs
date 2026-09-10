/**
 * llama-cpp-sys-2 0.1.156 forwards env vars whose names start with CMAKE_
 * (config.define(key, value)) and GGML_, then overwrites GGML_NATIVE from
 * RUSTFLAGS `target-cpu`. CMAKE_ARGS is unused.
 */

/** SSE4.2 without AVX2; keeps GGML_NATIVE off (not target-cpu=native). */
export const PORTABLE_SSE42_RUSTFLAG = '-C target-feature=+sse4.2';

/** Keep bundled ggml next to the sidecar; do not search Homebrew prefixes. */
export const MACOS_GGML_CMAKE_ENV = {
  CMAKE_MACOSX_RPATH: 'ON',
  CMAKE_BUILD_WITH_INSTALL_RPATH: 'ON',
  CMAKE_INSTALL_RPATH: '@loader_path',
  CMAKE_IGNORE_PREFIX_PATH: '/opt/homebrew;/usr/local',
};

/** Rust linker rpath so `aifs-worker-llm` loads ggml beside itself. */
export const MACOS_LOADER_RPATH_RUSTFLAG = '-C link-arg=-Wl,-rpath,@loader_path';

/** Drops `-C target-cpu=native` so llama-cpp-sys-2 leaves GGML_NATIVE=OFF. */
export function withoutTargetCpuNative(rustflags) {
  if (!rustflags) return '';
  return rustflags.replace(/(?:^|\s)-C\s+target-cpu=native\b/g, ' ').replace(/\s+/g, ' ').trim();
}

/** Appends a whole rustc flag chunk if it is not already present. */
export function appendRustflagChunk(existing, chunk) {
  const current = existing ?? '';
  if (!chunk) return current;
  if (current.includes(chunk)) return current;
  return [current, chunk].filter(Boolean).join(' ');
}

/** Named CMAKE_* env vars llama-cpp-sys-2 will define(). Empty when not packaging. */
export function packagedGgmlCmakeEnv({ platform, packaged }) {
  if (!packaged) return {};
  if (platform === 'darwin') return { ...MACOS_GGML_CMAKE_ENV };
  return {};
}

/**
 * Sets CMAKE_* and RUSTFLAGS for Tauri package builds. Local `pnpm llama`
 * leaves llama-cpp-2 defaults (`packaged: false`).
 */
export function applyPackagedGgmlEnv({
  env = process.env,
  platform = process.platform,
  packaged = false,
} = {}) {
  if (!packaged) return undefined;
  if (platform === 'win32' || platform === 'linux') {
    env.RUSTFLAGS = appendRustflagChunk(
      withoutTargetCpuNative(env.RUSTFLAGS),
      PORTABLE_SSE42_RUSTFLAG,
    );
  }
  if (platform === 'darwin') {
    Object.assign(env, MACOS_GGML_CMAKE_ENV);
    env.RUSTFLAGS = appendRustflagChunk(env.RUSTFLAGS, MACOS_LOADER_RPATH_RUSTFLAG);
  }
  return {
    CMAKE_INSTALL_RPATH: env.CMAKE_INSTALL_RPATH,
    CMAKE_IGNORE_PREFIX_PATH: env.CMAKE_IGNORE_PREFIX_PATH,
    RUSTFLAGS: env.RUSTFLAGS,
  };
}
