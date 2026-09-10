/** Accelerator Cargo features for `aifs-worker-llm` besides `llama`. */
export const LLM_ACCEL_FEATURES = ['cuda', 'vulkan', 'metal'];

const FEATURE_ALIASES = new Map([
  ['vulcan', 'vulkan'],
  ['mtl', 'metal'],
]);

/** Parse `AIFS_LLM_FEATURES` / script args into canonical llama.cpp extra features. */
export function parseLlmFeatures(raw) {
  const extras = [];
  const seen = new Set();
  for (const part of String(raw ?? '').split(/[,\s]+/)) {
    const token = part.trim().toLowerCase();
    if (!token || token === 'llama') continue;
    const feature = FEATURE_ALIASES.get(token) ?? token;
    if (!LLM_ACCEL_FEATURES.includes(feature) || seen.has(feature)) continue;
    seen.add(feature);
    extras.push(feature);
  }
  return extras;
}

/** True when argv is `cargo engine-llm` or `cargo build -p aifs-worker-llm`. */
export function isCargoEngineLlm(argv) {
  const tokens = argv.filter((arg) => arg !== '--packaged');
  if (tokens.some((arg) => arg === 'engine-llm')) return true;
  for (let i = 0; i < tokens.length; i += 1) {
    if (tokens[i] === '-p' && tokens[i + 1] === 'aifs-worker-llm') return true;
    if (tokens[i] === '--package' && tokens[i + 1] === 'aifs-worker-llm') return true;
    if (tokens[i] === '-p=aifs-worker-llm' || tokens[i] === '--package=aifs-worker-llm') {
      return true;
    }
  }
  return false;
}

/** True when cargo `--features` already lists `feature`. */
export function argvHasFeature(argv, feature) {
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--features' || arg === '-F') {
      if (featureListIncludes(argv[i + 1], feature)) return true;
      continue;
    }
    const assigned = arg.match(/^--features=(.*)$/);
    if (assigned && featureListIncludes(assigned[1], feature)) return true;
  }
  return false;
}

/**
 * Append `--features llama,<AIFS_LLM_FEATURES>` onto `cargo engine-llm` when missing.
 * Tauri `beforeDevCommand` stays `cargo engine-llm`; the env carries CUDA/Vulkan.
 */
export function appendEngineLlmFeatures(argv, env = process.env) {
  const extras = parseLlmFeatures(env.AIFS_LLM_FEATURES);
  if (extras.length === 0 || !isCargoEngineLlm(argv)) return argv;
  const missing = extras.filter((feature) => !argvHasFeature(argv, feature));
  if (missing.length === 0) return argv;
  return [...argv, '--features', ['llama', ...missing].join(',')];
}

/** llama.cpp on Linux expects g++ as the host compiler when CXX is unset. */
export function applyLinuxLlamaCxx({
  argv,
  env = process.env,
  platform = process.platform,
} = {}) {
  if (platform !== 'linux' || env.CXX || !isCargoEngineLlm(argv)) return undefined;
  env.CXX = 'g++';
  return 'g++';
}

function featureListIncludes(list, feature) {
  return String(list ?? '')
    .split(/[,\s]+/)
    .map((item) => item.trim())
    .includes(feature);
}
