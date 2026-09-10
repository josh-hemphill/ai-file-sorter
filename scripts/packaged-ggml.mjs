/** CMake flag so packaged Windows/Linux CPU ggml runs on SSE4.2-only hosts. */
export const GGML_NATIVE_OFF = '-DGGML_NATIVE=OFF';

/** Keep bundled ggml next to the sidecar; do not search Homebrew prefixes. */
export const MACOS_GGML_CMAKE_ARGS = [
  '-DCMAKE_MACOSX_RPATH=ON',
  '-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON',
  '-DCMAKE_INSTALL_RPATH=@loader_path',
  '-DCMAKE_IGNORE_PREFIX_PATH=/opt/homebrew;/usr/local',
];

/** Rust linker rpath so `aifs-worker-llm` loads ggml beside itself. */
export const MACOS_LOADER_RPATH_RUSTFLAG = '-C link-arg=-Wl,-rpath,@loader_path';

function tokenize(value) {
  if (!value) return [];
  return value.split(/\s+/).filter(Boolean);
}

/** Appends `extras` that are not already present. */
export function mergeFlagList(existing, extras) {
  const tokens = tokenize(existing);
  for (const extra of extras) {
    if (!tokens.includes(extra)) tokens.push(extra);
  }
  return tokens;
}

/** CMake `-D` flags for a packaged llama sidecar. Dev/`pnpm llama` stays empty. */
export function packagedGgmlCmakeFlags({ platform, packaged }) {
  if (!packaged) return [];
  if (platform === 'win32' || platform === 'linux') return [GGML_NATIVE_OFF];
  if (platform === 'darwin') return [...MACOS_GGML_CMAKE_ARGS];
  return [];
}

/**
 * Sets CMAKE_ARGS / RUSTFLAGS for Tauri package builds. Local `pnpm llama`
 * leaves llama-cpp-2 defaults (call with `packaged: false`).
 */
export function applyPackagedGgmlEnv({
  env = process.env,
  platform = process.platform,
  packaged = false,
} = {}) {
  if (!packaged) return undefined;
  const cmakeFlags = packagedGgmlCmakeFlags({ platform, packaged });
  if (cmakeFlags.length > 0) {
    env.CMAKE_ARGS = mergeFlagList(env.CMAKE_ARGS, cmakeFlags).join(' ');
  }
  if (platform === 'darwin') {
    env.RUSTFLAGS = mergeFlagList(env.RUSTFLAGS, [MACOS_LOADER_RPATH_RUSTFLAG]).join(
      ' ',
    );
  }
  return {
    CMAKE_ARGS: env.CMAKE_ARGS,
    RUSTFLAGS: env.RUSTFLAGS,
  };
}
