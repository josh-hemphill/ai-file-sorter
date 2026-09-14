/** Snapshot `aifs-worker-llm` + runtime libs into `llm-runtime/<accel>/` without clobbering siblings. */

import { copyFileSync, mkdirSync, readdirSync, rmSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseLlmFeatures } from './llm-features.mjs';

export const LLM_RUNTIME_DIR = 'llm-runtime';

/** MSVC cmake-rs nests ggml-cuda.dll under `out/bin/Release` (and similar). */
const LLAMA_OUT_MAX_DEPTH = 6;
const SKIP_LLAMA_OUT_DIRS = new Set([
  'cmakefiles',
  'cmaketmp',
  '.git',
  'compileridcuda',
  'compileridcxx',
  'compileridc',
]);

const LIB_PREFIXES = [
  'ggml',
  'llama',
  'mtmd',
  'cudart',
  'cublas',
  'nvrtc',
  'nvjitlink',
  'vulkan',
];

/** True when `name` is a native library the LLM worker loads. */
export function isWorkerRuntimeLib(name) {
  const lower = String(name).toLowerCase();
  if (lower === 'nvcuda.dll' || lower.startsWith('libcuda.')) return false;
  if (!isNativeLibName(lower)) return false;
  const stem = runtimeLibStem(lower);
  return LIB_PREFIXES.some((prefix) => stem.startsWith(prefix));
}

/** Infer payload folder from library filenames (`ggml-cuda` / `ggml-vulkan` / cpu). */
export function inferAccelFromLibNames(names) {
  const list = [...names];
  if (list.some((name) => runtimeLibStem(name).startsWith('ggml-cuda'))) return 'cuda';
  if (list.some((name) => runtimeLibStem(name).startsWith('ggml-vulkan'))) return 'vulkan';
  return 'cpu';
}

/** `root/llm-runtime/<accel>`. */
export function llmPayloadDir(root, accel) {
  return join(root, LLM_RUNTIME_DIR, accel);
}

/**
 * Copy worker + libs from `srcDir` into each `runtimeRoot/llm-runtime/<accel>/`.
 * Does not delete sibling accelerator folders. Returns `{ accel, copiedLibNames }`.
 */
export function stageLlmPayloadFromDir({
  srcDir,
  runtimeRoots,
  cudaRoot,
  expectedAccel,
} = {}) {
  const collected = collectRuntimeLibs(srcDir);
  const accel = inferAccelFromLibNames(collected.keys());
  assertExpectedAccel(expectedAccel, accel, collected, srcDir);
  if (accel === 'cuda' && cudaRoot) {
    collectRuntimeLibsFrom(join(cudaRoot, 'bin'), collected);
  }
  const worker = findWorkerBinary(srcDir);
  for (const runtimeRoot of runtimeRoots) {
    const destDir = llmPayloadDir(runtimeRoot, accel);
    mkdirSync(destDir, { recursive: true });
    if (worker) {
      copyFileSync(worker.path, join(destDir, worker.name));
    }
    for (const [name, src] of collected) {
      copyFileSync(src, join(destDir, name));
    }
    pruneStaleLibs(destDir, collected);
  }
  return { accel, copiedLibNames: [...collected.keys()].sort() };
}

/** `cuda` / `vulkan` from `AIFS_LLM_FEATURES`; CPU llama leaves this unset. */
export function expectedAccelFromEnv(env = process.env) {
  const extras = parseLlmFeatures(env.AIFS_LLM_FEATURES);
  if (extras.includes('cuda')) return 'cuda';
  if (extras.includes('vulkan')) return 'vulkan';
  return undefined;
}

/** One-line staging summary so CUDA success names `ggml-cuda.dll`. */
export function formatStagedPayloadLog(accel, copiedLibNames) {
  const libs = [...copiedLibNames].sort();
  const detail = libs.length > 0 ? ` with ${libs.join(', ')}` : ' (worker only)';
  return `aifs: staged llm-runtime/${accel}${detail} (siblings left in place)`;
}

function requiredPluginForAccel(accel) {
  if (accel === 'cuda') return 'ggml-cuda';
  if (accel === 'vulkan') return 'ggml-vulkan';
  return undefined;
}

function assertExpectedAccel(expectedAccel, accel, collected, srcDir) {
  const plugin = requiredPluginForAccel(expectedAccel);
  if (!plugin || expectedAccel === accel) return;
  const found = [...collected.keys()].sort().join(', ') || '(none)';
  throw new Error(
    `aifs: AIFS_LLM_FEATURES=${expectedAccel} but inferred ${accel} (missing ${plugin}). Found: ${found}. Searched ${srcDir}, deps/, and nested build/llama-cpp-*/out (MSVC cmake uses out/bin/Release or out/build/bin/Release). If llama-cpp-sys-2 was cached without CUDA, run cargo clean -p llama-cpp-sys-2 and rebuild with pnpm llama:cuda.`,
  );
}

function collectRuntimeLibs(srcDir) {
  const files = new Map();
  collectRuntimeLibsFrom(srcDir, files);
  collectRuntimeLibsFrom(join(srcDir, 'deps'), files);
  collectRuntimeLibsFromLlamaBuildOut(srcDir, files);
  return files;
}

function collectRuntimeLibsFromLlamaBuildOut(targetDir, files) {
  let entries;
  try {
    entries = readdirSync(join(targetDir, 'build'), { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    if (!String(entry.name).toLowerCase().startsWith('llama-cpp')) continue;
    collectRuntimeLibsNested(join(targetDir, 'build', entry.name, 'out'), files, 0);
  }
}

function collectRuntimeLibsNested(dir, files, depth) {
  if (depth > LLAMA_OUT_MAX_DEPTH) return;
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (SKIP_LLAMA_OUT_DIRS.has(String(entry.name).toLowerCase())) continue;
      collectRuntimeLibsNested(path, files, depth + 1);
      continue;
    }
    if (!isWorkerRuntimeLib(entry.name)) continue;
    try {
      if (!statSync(path).isFile()) continue;
    } catch {
      continue;
    }
    files.set(entry.name, path);
  }
}

function collectRuntimeLibsFrom(dir, files) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (!isWorkerRuntimeLib(entry.name)) continue;
    const path = join(dir, entry.name);
    try {
      if (!statSync(path).isFile()) continue;
    } catch {
      continue;
    }
    files.set(entry.name, path);
  }
}

function pruneStaleLibs(destDir, keep) {
  let entries;
  try {
    entries = readdirSync(destDir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (!isWorkerRuntimeLib(entry.name)) continue;
    const path = join(destDir, entry.name);
    try {
      if (!statSync(path).isFile()) continue;
    } catch {
      continue;
    }
    if (!keep.has(entry.name)) {
      rmSync(path);
    }
  }
}

function findWorkerBinary(srcDir) {
  for (const name of ['aifs-worker-llm', 'aifs-worker-llm.exe']) {
    const path = join(srcDir, name);
    try {
      if (statSync(path).isFile() && statSync(path).size > 0) {
        return { path, name };
      }
    } catch {
      continue;
    }
  }
  return undefined;
}

function isNativeLibName(lower) {
  return (
    lower.endsWith('.dll') ||
    lower.endsWith('.dylib') ||
    lower.endsWith('.so') ||
    lower.includes('.so.')
  );
}

function runtimeLibStem(name) {
  const lower = String(name).toLowerCase();
  const file = lower.split(/[/\\]/).pop() ?? lower;
  const stripped = file.startsWith('lib') ? file.slice(3) : file;
  if (stripped.endsWith('.dll')) return stripped.slice(0, -4);
  if (stripped.endsWith('.dylib')) return stripped.slice(0, -6);
  const so = stripped.indexOf('.so');
  if (so >= 0) return stripped.slice(0, so);
  return stripped;
}

function repoRootFromScript() {
  return dirname(dirname(fileURLToPath(import.meta.url)));
}

function profileFromArgv(argv) {
  return argv.includes('--release') ? 'release' : 'debug';
}

export function defaultStagePlan({
  argv = process.argv.slice(2),
  cwd = repoRootFromScript(),
  env = process.env,
} = {}) {
  const profile = profileFromArgv(argv);
  const srcDir = join(cwd, 'target', profile);
  return {
    srcDir,
    runtimeRoots: [srcDir, join(cwd, 'apps/desktop/src-tauri/resources')],
    cudaRoot: env.CUDA_PATH,
    expectedAccel: expectedAccelFromEnv(env),
  };
}

/** True when the wrapper should snapshot after a real `cargo engine-llm`. */
export function shouldStageAfterEngineLlm(command, argv) {
  return command === 'cargo' && argv.includes('engine-llm');
}

const invokedDirectly =
  process.argv[1] &&
  fileURLToPath(import.meta.url) === process.argv[1];

if (invokedDirectly) {
  const { accel, copiedLibNames } = stageLlmPayloadFromDir(defaultStagePlan());
  console.error(formatStagedPayloadLog(accel, copiedLibNames));
}
