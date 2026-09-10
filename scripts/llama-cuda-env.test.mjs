import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  DEFAULT_CUDA_CMAKE_JOBS,
  applyLlamaCudaBuildEnv,
  argvRequestsCuda,
  cmakeBuildParallelLevelForCuda,
  cmakeCudaArchitecturesFromSmi,
  computeCapToCmakeArch,
} from './llama-cuda-env.mjs';

const here = dirname(fileURLToPath(import.meta.url));

test('argvRequestsCuda reads cargo --features and AIFS_LLM_FEATURES', () => {
  assert.equal(argvRequestsCuda(['cargo', 'engine-llm'], {}), false);
  assert.equal(
    argvRequestsCuda(['cargo', 'build', '-p', 'aifs-worker-llm', '--features', 'llama'], {}),
    false,
  );
  assert.equal(
    argvRequestsCuda(
      ['cargo', 'build', '-p', 'aifs-worker-llm', '--features', 'llama,cuda'],
      {},
    ),
    true,
  );
  assert.equal(argvRequestsCuda(['cargo', 'engine-llm', '--features=llama,cuda'], {}), true);
  assert.equal(argvRequestsCuda(['cargo', 'engine-llm', '-F', 'cuda'], {}), true);
  assert.equal(argvRequestsCuda(['cargo', 'engine-llm'], { AIFS_LLM_FEATURES: 'cuda,vulkan' }), true);
  assert.equal(
    argvRequestsCuda(['/opt/cuda/bin/nvcc', 'engine-llm'], {}),
    false,
  );
});

test('computeCapToCmakeArch maps nvidia-smi caps to llama.cpp CMake archs', () => {
  assert.equal(computeCapToCmakeArch('8.6'), '86');
  assert.equal(computeCapToCmakeArch('8.9'), '89');
  assert.equal(computeCapToCmakeArch('7.5'), '75');
  assert.equal(computeCapToCmakeArch('9.0'), '90');
  assert.equal(computeCapToCmakeArch('12.0'), '120a');
  assert.equal(computeCapToCmakeArch('12.1'), '121a');
  assert.equal(computeCapToCmakeArch(' 8.6 '), '86');
  assert.equal(computeCapToCmakeArch('3.0'), undefined);
  assert.equal(computeCapToCmakeArch('not-a-gpu'), undefined);
});

test('cmakeCudaArchitecturesFromSmi unique-joins compute caps', () => {
  assert.equal(cmakeCudaArchitecturesFromSmi('8.6\n'), '86');
  assert.equal(cmakeCudaArchitecturesFromSmi('8.6\n8.6\n8.9\n'), '86;89');
  assert.equal(cmakeCudaArchitecturesFromSmi('8.6, NVIDIA\n'), '86');
  assert.equal(cmakeCudaArchitecturesFromSmi(''), undefined);
});

test('cmakeBuildParallelLevelForCuda keeps an explicit value and caps otherwise', () => {
  assert.equal(
    cmakeBuildParallelLevelForCuda({ existing: '2', cpuCount: 32 }),
    '2',
  );
  assert.equal(cmakeBuildParallelLevelForCuda({ cpuCount: 32 }), String(DEFAULT_CUDA_CMAKE_JOBS));
  assert.equal(cmakeBuildParallelLevelForCuda({ cpuCount: 2 }), '2');
  assert.equal(cmakeBuildParallelLevelForCuda({ cpuCount: 0 }), '1');
});

test('applyLlamaCudaBuildEnv is a no-op without the cuda feature', () => {
  const env = {};
  const messages = [];
  assert.equal(
    applyLlamaCudaBuildEnv({
      argv: ['cargo', 'engine-llm'],
      env,
      detectArchitectures: () => {
        throw new Error('nvidia-smi must not run without cuda');
      },
      log: (message) => messages.push(message),
    }),
    undefined,
  );
  assert.equal(env.CMAKE_CUDA_ARCHITECTURES, undefined);
  assert.equal(messages.length, 0);
});

test('applyLlamaCudaBuildEnv pins a detected SM and caps cmake jobs', () => {
  const env = {};
  const messages = [];
  const applied = applyLlamaCudaBuildEnv({
    argv: ['cargo', 'engine-llm', '--features', 'llama,cuda'],
    env,
    platform: 'darwin',
    detectArchitectures: () => '86',
    cpuCount: 16,
    log: (message) => messages.push(message),
  });
  assert.deepEqual(applied, {
    CMAKE_CUDA_ARCHITECTURES: '86',
    CMAKE_BUILD_PARALLEL_LEVEL: '4',
  });
  assert.equal(env.CMAKE_CUDA_ARCHITECTURES, '86');
  assert.equal(env.CMAKE_BUILD_PARALLEL_LEVEL, '4');
  assert.equal(env.CXX, undefined);
  assert.match(messages[0], /CMAKE_CUDA_ARCHITECTURES=86/);
  assert.match(messages[0], /no Cargo progress/);
});

test('applyLlamaCudaBuildEnv preserves caller CMAKE_* and sets CXX on Linux', () => {
  const env = {
    CMAKE_CUDA_ARCHITECTURES: '89',
    CMAKE_BUILD_PARALLEL_LEVEL: '1',
  };
  const applied = applyLlamaCudaBuildEnv({
    argv: ['cargo', 'build', '--features=cuda'],
    env,
    platform: 'linux',
    detectArchitectures: () => {
      throw new Error('must not probe when CMAKE_CUDA_ARCHITECTURES is set');
    },
    cpuCount: 16,
    log: () => {},
  });
  assert.deepEqual(applied, { CXX: 'g++' });
  assert.equal(env.CMAKE_CUDA_ARCHITECTURES, '89');
  assert.equal(env.CMAKE_BUILD_PARALLEL_LEVEL, '1');
  assert.equal(env.CXX, 'g++');
});

test('applyLlamaCudaBuildEnv warns when no GPU SM is known', () => {
  const env = {};
  const messages = [];
  applyLlamaCudaBuildEnv({
    argv: ['cargo', 'engine-llm', '--features', 'cuda'],
    env,
    platform: 'win32',
    detectArchitectures: () => undefined,
    cpuCount: 8,
    log: (message) => messages.push(message),
  });
  assert.equal(env.CMAKE_CUDA_ARCHITECTURES, undefined);
  assert.equal(env.CMAKE_BUILD_PARALLEL_LEVEL, '4');
  assert.match(messages[0], /Maxwell through Blackwell/);
});

test('with-cmake-generator.mjs caps CMAKE_BUILD_PARALLEL_LEVEL for cuda features', () => {
  const env = { ...process.env };
  delete env.CMAKE_BUILD_PARALLEL_LEVEL;
  delete env.CMAKE_CUDA_ARCHITECTURES;
  delete env.AIFS_LLM_FEATURES;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      process.execPath,
      '-e',
      'process.exit(process.env.CMAKE_BUILD_PARALLEL_LEVEL ? 0 : 1)',
      '--',
      '--features',
      'llama,cuda',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0);
  assert.match(result.stderr, /llama-cpp-sys-2 CUDA compile/);
});

test('with-cmake-generator.mjs does not cap cmake jobs for CPU llama', () => {
  const env = { ...process.env };
  delete env.CMAKE_BUILD_PARALLEL_LEVEL;
  delete env.AIFS_LLM_FEATURES;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      process.execPath,
      '-e',
      'process.exit(process.env.CMAKE_BUILD_PARALLEL_LEVEL ? 1 : 0)',
      '--',
      '--features',
      'llama',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0);
});
