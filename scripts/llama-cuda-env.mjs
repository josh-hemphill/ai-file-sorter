import { spawnSync } from 'node:child_process';
import { availableParallelism } from 'node:os';

/** nvcc RAM use makes all-core cmake jobs swap; cap when unset. */
export const DEFAULT_CUDA_CMAKE_JOBS = 4;
/** MSVC + nvcc at high `-j` writes truncated .obj files (LNK1136). */
export const WINDOWS_CUDA_CMAKE_JOBS = 2;

/** Default cmake job cap for a CUDA llama.cpp build on this OS. */
export function cudaCmakeJobCap(platform = process.platform) {
  return platform === 'win32' ? WINDOWS_CUDA_CMAKE_JOBS : DEFAULT_CUDA_CMAKE_JOBS;
}

/** True when cargo `--features` or `AIFS_LLM_FEATURES` includes `cuda`. */
export function argvRequestsCuda(argv, env = process.env) {
  if (featureListIncludesCuda(env.AIFS_LLM_FEATURES)) return true;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--features' || arg === '-F') {
      if (featureListIncludesCuda(argv[i + 1])) return true;
      continue;
    }
    const assigned = arg.match(/^--features=(.*)$/);
    if (assigned && featureListIncludesCuda(assigned[1])) return true;
  }
  return false;
}

/** Convert `nvidia-smi` compute_cap (`8.6`) to a CMake CUDA architecture (`86`). */
export function computeCapToCmakeArch(computeCap) {
  const match = String(computeCap ?? '')
    .trim()
    .match(/^(\d+)\.(\d+)$/);
  if (!match) return undefined;
  const major = Number(match[1]);
  const minor = Number(match[2]);
  if (!Number.isInteger(major) || !Number.isInteger(minor)) return undefined;
  const code = major * 10 + minor;
  if (code < 50) return undefined;
  // llama.cpp maps 12X → 12Xa (Blackwell FP4 is not forwards-compatible).
  if (code >= 120) return `${code}a`;
  return String(code);
}

/** Parse `nvidia-smi --query-gpu=compute_cap --format=csv,noheader` into unique CMake archs. */
export function cmakeCudaArchitecturesFromSmi(smiOutput) {
  const archs = [];
  const seen = new Set();
  for (const line of String(smiOutput ?? '').split(/\r?\n/)) {
    const arch = computeCapToCmakeArch(line.replace(/,.*/, '').trim());
    if (!arch || seen.has(arch)) continue;
    seen.add(arch);
    archs.push(arch);
  }
  return archs.length > 0 ? archs.join(';') : undefined;
}

/** Cap CUDA cmake jobs unless `CMAKE_BUILD_PARALLEL_LEVEL` is already set. */
export function cmakeBuildParallelLevelForCuda({
  existing,
  cpuCount = availableParallelism(),
  platform = process.platform,
  maxJobs = cudaCmakeJobCap(platform),
} = {}) {
  if (existing) return existing;
  const cpus = Number(cpuCount);
  const usable = Number.isFinite(cpus) && cpus > 0 ? Math.floor(cpus) : 1;
  return String(Math.max(1, Math.min(maxJobs, usable)));
}

/**
 * Pin one GPU SM and cap cmake jobs for llama-cpp-sys-2 CUDA builds.
 * Cargo shows no progress while nvcc runs; cmake-rs swallows output unless CMAKE_VERBOSE is set.
 */
export function applyLlamaCudaBuildEnv({
  argv = process.argv.slice(2),
  env = process.env,
  platform = process.platform,
  detectArchitectures = detectCudaArchitecturesFromNvidiaSmi,
  cpuCount = availableParallelism(),
  log = console.error,
} = {}) {
  if (!argvRequestsCuda(argv, env)) return undefined;

  const applied = {};
  if (!env.CMAKE_CUDA_ARCHITECTURES) {
    const detected = detectArchitectures();
    if (detected) {
      env.CMAKE_CUDA_ARCHITECTURES = detected;
      applied.CMAKE_CUDA_ARCHITECTURES = detected;
    }
  }
  const maxJobs = cudaCmakeJobCap(platform);
  if (!env.CMAKE_BUILD_PARALLEL_LEVEL) {
    env.CMAKE_BUILD_PARALLEL_LEVEL = cmakeBuildParallelLevelForCuda({
      cpuCount,
      maxJobs,
    });
    applied.CMAKE_BUILD_PARALLEL_LEVEL = env.CMAKE_BUILD_PARALLEL_LEVEL;
  } else if (platform === 'win32') {
    const requested = Number(env.CMAKE_BUILD_PARALLEL_LEVEL);
    if (Number.isFinite(requested) && requested > maxJobs) {
      log(
        `aifs: CMAKE_BUILD_PARALLEL_LEVEL=${env.CMAKE_BUILD_PARALLEL_LEVEL} with CUDA on Windows often yields LNK1136 (corrupt ggml-cuda .obj). Use ${maxJobs} jobs, cargo clean -p llama-cpp-sys-2, and a local NTFS CARGO_TARGET_DIR if the repo is on a network share.`,
      );
    }
  }
  if (platform === 'linux' && !env.CXX) {
    env.CXX = 'g++';
    applied.CXX = 'g++';
  }

  const arch = env.CMAKE_CUDA_ARCHITECTURES;
  const jobs = env.CMAKE_BUILD_PARALLEL_LEVEL;
  if (arch) {
    log(
      `aifs: llama-cpp-sys-2 CUDA compile has no Cargo progress (cmake-rs hides nvcc). CMAKE_CUDA_ARCHITECTURES=${arch} CMAKE_BUILD_PARALLEL_LEVEL=${jobs}. Expect several minutes; CMAKE_VERBOSE=1 prints cmake. If a multi-arch compile already started, cargo clean -p llama-cpp-sys-2 first.`,
    );
  } else {
    log(
      `aifs: llama-cpp-sys-2 CUDA compile has no Cargo progress. Unset CMAKE_CUDA_ARCHITECTURES builds Maxwell through Blackwell and can look hung for an hour. Set CMAKE_CUDA_ARCHITECTURES to your GPU SM (86 RTX 30, 89 RTX 40, 75 Turing, 120a Blackwell) and CMAKE_BUILD_PARALLEL_LEVEL=${maxJobs}. CMAKE_VERBOSE=1 prints cmake. cargo clean -p llama-cpp-sys-2 if a fat compile already started. Windows LNK1136 (corrupt .obj) means too many cmake jobs or a network-share target dir.`,
    );
  }
  return applied;
}

function featureListIncludesCuda(list) {
  return String(list ?? '')
    .split(/[,\s]+/)
    .map((feature) => feature.trim())
    .includes('cuda');
}

function detectCudaArchitecturesFromNvidiaSmi() {
  const result = spawnSync(
    'nvidia-smi',
    ['--query-gpu=compute_cap', '--format=csv,noheader'],
    { encoding: 'utf8', windowsHide: true },
  );
  if (result.status !== 0) return undefined;
  return cmakeCudaArchitecturesFromSmi(result.stdout);
}
