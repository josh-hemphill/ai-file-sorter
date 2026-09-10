/**
 * llama-cpp-sys-2 0.1.156 forwards env vars whose names start with CMAKE_
 * (config.define(key, value)) and GGML_, then overwrites GGML_NATIVE from
 * RUSTFLAGS `target-cpu` and turns matching GGML_AVX* ON from
 * CARGO_CFG_TARGET_FEATURE. CMAKE_ARGS is unused. Vendored ggml treats
 * GGML_NATIVE=OFF on a non-cross build as INS_ENB=ON, so AVX2 defaults on
 * unless GGML_AVX2 is defined OFF.
 */

/** SSE4.2 without AVX2; later minus flags win over a hotter ambient RUSTFLAGS. */
export const PORTABLE_SSE42_RUSTFLAG =
  '-C target-feature=+sse4.2,-avx,-avx2,-fma,-f16c,-bmi2';

/**
 * ISA cache entries llama-cpp-sys-2 forwards before it may set some ON.
 * SSE4.2 stays on; host-tune flags stay off so packaged CPUs without AVX2 run.
 */
export const PORTABLE_X86_GGML_ENV = {
  GGML_SSE42: 'ON',
  GGML_AVX: 'OFF',
  GGML_AVX_VNNI: 'OFF',
  GGML_AVX2: 'OFF',
  GGML_BMI2: 'OFF',
  GGML_AVX512: 'OFF',
  GGML_AVX512_VBMI: 'OFF',
  GGML_AVX512_VNNI: 'OFF',
  GGML_AVX512_BF16: 'OFF',
  GGML_FMA: 'OFF',
  GGML_F16C: 'OFF',
};

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
  return rustflags
    .replace(/(?:^|\s)-C\s*target-cpu=native\b/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

/** Appends a whole rustc flag chunk if it is not already present. */
export function appendRustflagChunk(existing, chunk) {
  const current = existing ?? '';
  if (!chunk) return current;
  if (current.includes(chunk)) return current;
  return [current, chunk].filter(Boolean).join(' ');
}

/** Named CMAKE_* / GGML_* env vars llama-cpp-sys-2 will define(). Empty when not packaging. */
export function packagedGgmlCmakeEnv({ platform, packaged }) {
  if (!packaged) return {};
  if (platform === 'darwin') return { ...MACOS_GGML_CMAKE_ENV };
  if (platform === 'win32' || platform === 'linux') return { ...PORTABLE_X86_GGML_ENV };
  return {};
}

/**
 * Sets CMAKE_* / GGML_* and RUSTFLAGS for Tauri package builds. Local `pnpm llama`
 * leaves llama-cpp-2 defaults (`packaged: false`).
 */
export function applyPackagedGgmlEnv({
  env = process.env,
  platform = process.platform,
  packaged = false,
} = {}) {
  if (!packaged) return undefined;
  const cmake = packagedGgmlCmakeEnv({ platform, packaged: true });
  Object.assign(env, cmake);
  if (platform === 'win32' || platform === 'linux') {
    env.RUSTFLAGS = appendRustflagChunk(
      withoutTargetCpuNative(env.RUSTFLAGS),
      PORTABLE_SSE42_RUSTFLAG,
    );
  }
  if (platform === 'darwin') {
    env.RUSTFLAGS = appendRustflagChunk(env.RUSTFLAGS, MACOS_LOADER_RPATH_RUSTFLAG);
  }
  return {
    ...cmake,
    RUSTFLAGS: env.RUSTFLAGS,
  };
}
