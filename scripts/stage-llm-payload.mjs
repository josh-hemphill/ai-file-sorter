/** Snapshot `aifs-worker-llm` + runtime libs into `llm-runtime/<accel>/` without clobbering siblings. */

import { copyFileSync, mkdirSync, readdirSync, rmSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const LLM_RUNTIME_DIR = 'llm-runtime';

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
 * Does not delete sibling accelerator folders. Returns the inferred accel.
 */
export function stageLlmPayloadFromDir({
  srcDir,
  runtimeRoots,
  cudaRoot,
} = {}) {
  const collected = collectRuntimeLibs(srcDir);
  const accel = inferAccelFromLibNames(collected.keys());
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
  return accel;
}

function collectRuntimeLibs(srcDir) {
  const files = new Map();
  collectRuntimeLibsFrom(srcDir, files);
  collectRuntimeLibsFrom(join(srcDir, 'deps'), files);
  return files;
}

function collectRuntimeLibsFrom(dir, files) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (!entry.isFile() || !isWorkerRuntimeLib(entry.name)) continue;
    files.set(entry.name, join(dir, entry.name));
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
    if (!entry.isFile() || !isWorkerRuntimeLib(entry.name)) continue;
    if (!keep.has(entry.name)) {
      rmSync(join(destDir, entry.name));
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
  };
}

const invokedDirectly =
  process.argv[1] &&
  fileURLToPath(import.meta.url) === process.argv[1];

if (invokedDirectly) {
  const accel = stageLlmPayloadFromDir(defaultStagePlan());
  console.error(`aifs: staged llm-runtime/${accel} (siblings left in place)`);
}
