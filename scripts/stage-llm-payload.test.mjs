import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import {
  defaultStagePlan,
  expectedAccelFromEnv,
  formatStagedPayloadLog,
  inferAccelFromLibNames,
  isWorkerRuntimeLib,
  llmPayloadDir,
  shouldStageAfterEngineLlm,
  stageLlmPayloadFromDir,
} from './stage-llm-payload.mjs';

test('runtime lib matcher copies ggml plugins and skips nvcuda', () => {
  assert.equal(isWorkerRuntimeLib('ggml.dll'), true);
  assert.equal(isWorkerRuntimeLib('ggml-cuda.dll'), true);
  assert.equal(isWorkerRuntimeLib('libllama.so.0'), true);
  assert.equal(isWorkerRuntimeLib('cudart64_12.dll'), true);
  assert.equal(isWorkerRuntimeLib('nvcuda.dll'), false);
  assert.equal(isWorkerRuntimeLib('libcuda.so.1'), false);
  assert.equal(isWorkerRuntimeLib('aifs-worker-llm'), false);
});

test('inferAccelFromLibNames prefers cuda then vulkan then cpu', () => {
  assert.equal(inferAccelFromLibNames(['llama.dll', 'ggml.dll']), 'cpu');
  assert.equal(inferAccelFromLibNames(['llama.dll', 'ggml-vulkan.dll']), 'vulkan');
  assert.equal(
    inferAccelFromLibNames(['llama.dll', 'ggml-vulkan.dll', 'ggml-cuda.dll']),
    'cuda',
  );
});

test('cpu stage leaves an existing cuda payload in place', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  mkdirSync(src);
  writeFileSync(join(src, 'aifs-worker-llm'), 'cpu-worker');
  writeFileSync(join(src, 'llama.dll'), 'llama');
  writeFileSync(join(src, 'ggml.dll'), 'ggml');
  const cudaDest = llmPayloadDir(resources, 'cuda');
  mkdirSync(cudaDest, { recursive: true });
  writeFileSync(join(cudaDest, 'ggml-cuda.dll'), 'keep');
  writeFileSync(join(cudaDest, 'aifs-worker-llm'), 'cuda-worker');
  const { accel } = stageLlmPayloadFromDir({
    srcDir: src,
    runtimeRoots: [resources],
  });
  assert.equal(accel, 'cpu');
  assert.equal(readFileSync(join(llmPayloadDir(resources, 'cpu'), 'ggml.dll'), 'utf8'), 'ggml');
  assert.equal(readFileSync(join(cudaDest, 'ggml-cuda.dll'), 'utf8'), 'keep');
  assert.equal(readFileSync(join(cudaDest, 'aifs-worker-llm'), 'utf8'), 'cuda-worker');
  rmSync(root, { recursive: true, force: true });
});

test('cuda stage lands under cuda and does not flatten', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-cuda-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  const toolkit = join(root, 'toolkit');
  mkdirSync(src);
  mkdirSync(join(toolkit, 'bin'), { recursive: true });
  writeFileSync(join(src, 'aifs-worker-llm'), 'cuda-worker');
  writeFileSync(join(src, 'llama.dll'), 'llama');
  writeFileSync(join(src, 'ggml.dll'), 'ggml');
  writeFileSync(join(src, 'ggml-cuda.dll'), 'cuda');
  writeFileSync(join(toolkit, 'bin', 'cudart64_12.dll'), 'cudart');
  writeFileSync(join(toolkit, 'bin', 'nvcuda.dll'), 'driver');
  const { accel, copiedLibNames } = stageLlmPayloadFromDir({
    srcDir: src,
    runtimeRoots: [resources],
    cudaRoot: toolkit,
  });
  assert.equal(accel, 'cuda');
  assert.ok(copiedLibNames.includes('ggml-cuda.dll'));
  assert.ok(copiedLibNames.includes('cudart64_12.dll'));
  const dest = llmPayloadDir(resources, 'cuda');
  assert.equal(readFileSync(join(dest, 'ggml-cuda.dll'), 'utf8'), 'cuda');
  assert.equal(readFileSync(join(dest, 'cudart64_12.dll'), 'utf8'), 'cudart');
  assert.equal(readFileSync(join(dest, 'aifs-worker-llm'), 'utf8'), 'cuda-worker');
  assert.equal(existsSync(join(dest, 'nvcuda.dll')), false);
  rmSync(root, { recursive: true, force: true });
});

test('cuda plugin only in llama-cpp out still stages under cuda', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-out-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  mkdirSync(join(src, 'build', 'llama-cpp-sys-2-deadbeef', 'out'), { recursive: true });
  writeFileSync(join(src, 'aifs-worker-llm'), 'worker');
  writeFileSync(join(src, 'llama.dll'), 'llama');
  writeFileSync(join(src, 'ggml.dll'), 'ggml');
  writeFileSync(
    join(src, 'build', 'llama-cpp-sys-2-deadbeef', 'out', 'ggml-cuda.dll'),
    'cuda',
  );
  const { accel } = stageLlmPayloadFromDir({ srcDir: src, runtimeRoots: [resources] });
  assert.equal(accel, 'cuda');
  assert.equal(
    readFileSync(join(llmPayloadDir(resources, 'cuda'), 'ggml-cuda.dll'), 'utf8'),
    'cuda',
  );
  rmSync(root, { recursive: true, force: true });
});

test('cuda plugin only in nested MSVC llama-cpp out still stages under cuda', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-nested-out-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  const release = join(
    src,
    'build',
    'llama-cpp-sys-2-deadbeef',
    'out',
    'build',
    'bin',
    'Release',
  );
  mkdirSync(release, { recursive: true });
  mkdirSync(join(src, 'build', 'llama-cpp-sys-2-deadbeef', 'out', 'CMakeFiles'), {
    recursive: true,
  });
  writeFileSync(join(src, 'aifs-worker-llm'), 'worker');
  writeFileSync(join(release, 'llama.dll'), 'llama');
  writeFileSync(join(release, 'ggml.dll'), 'ggml');
  writeFileSync(join(release, 'ggml-cuda.dll'), 'cuda');
  writeFileSync(
    join(src, 'build', 'llama-cpp-sys-2-deadbeef', 'out', 'CMakeFiles', 'ggml-vulkan.dll'),
    'skip',
  );
  const { accel, copiedLibNames } = stageLlmPayloadFromDir({
    srcDir: src,
    runtimeRoots: [resources],
  });
  assert.equal(accel, 'cuda');
  assert.deepEqual(copiedLibNames, ['ggml-cuda.dll', 'ggml.dll', 'llama.dll']);
  assert.equal(
    readFileSync(join(llmPayloadDir(resources, 'cuda'), 'ggml-cuda.dll'), 'utf8'),
    'cuda',
  );
  assert.equal(existsSync(join(llmPayloadDir(resources, 'cuda'), 'ggml-vulkan.dll')), false);
  rmSync(root, { recursive: true, force: true });
});

test('expected cuda without plugin does not stage cpu', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-expected-cuda-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  mkdirSync(src);
  writeFileSync(join(src, 'aifs-worker-llm'), 'worker');
  writeFileSync(join(src, 'llama.dll'), 'llama');
  writeFileSync(join(src, 'ggml.dll'), 'ggml');
  assert.throws(
    () =>
      stageLlmPayloadFromDir({
        srcDir: src,
        runtimeRoots: [resources],
        expectedAccel: 'cuda',
      }),
    /missing ggml-cuda/,
  );
  assert.equal(existsSync(llmPayloadDir(resources, 'cpu')), false);
  assert.equal(existsSync(llmPayloadDir(resources, 'cuda')), false);
  rmSync(root, { recursive: true, force: true });
});

test('expectedAccelFromEnv reads AIFS_LLM_FEATURES', () => {
  assert.equal(expectedAccelFromEnv({ AIFS_LLM_FEATURES: 'cuda' }), 'cuda');
  assert.equal(expectedAccelFromEnv({ AIFS_LLM_FEATURES: 'vulcan' }), 'vulkan');
  assert.equal(expectedAccelFromEnv({}), undefined);
  assert.equal(
    defaultStagePlan({
      argv: ['engine-llm'],
      cwd: '/tmp/repo',
      env: { AIFS_LLM_FEATURES: 'cuda' },
    }).expectedAccel,
    'cuda',
  );
});

test('formatStagedPayloadLog names copied CUDA libs', () => {
  assert.equal(
    formatStagedPayloadLog('cuda', ['llama.dll', 'ggml-cuda.dll']),
    'aifs: staged llm-runtime/cuda with ggml-cuda.dll, llama.dll (siblings left in place)',
  );
  assert.equal(
    formatStagedPayloadLog('cpu', []),
    'aifs: staged llm-runtime/cpu (worker only) (siblings left in place)',
  );
});

test('cuda plugin only in deps still stages under cuda', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-deps-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  mkdirSync(join(src, 'deps'), { recursive: true });
  writeFileSync(join(src, 'aifs-worker-llm'), 'worker');
  writeFileSync(join(src, 'llama.dll'), 'llama');
  writeFileSync(join(src, 'ggml.dll'), 'ggml');
  writeFileSync(join(src, 'deps', 'ggml-cuda.dll'), 'cuda');
  const { accel } = stageLlmPayloadFromDir({ srcDir: src, runtimeRoots: [resources] });
  assert.equal(accel, 'cuda');
  assert.equal(
    readFileSync(join(llmPayloadDir(resources, 'cuda'), 'ggml-cuda.dll'), 'utf8'),
    'cuda',
  );
  assert.equal(existsSync(llmPayloadDir(resources, 'cpu')), false);
  rmSync(root, { recursive: true, force: true });
});

test('cpu stage does not copy CUDA toolkit libs', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-toolkit-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  const toolkit = join(root, 'toolkit');
  mkdirSync(src);
  mkdirSync(join(toolkit, 'bin'), { recursive: true });
  writeFileSync(join(src, 'aifs-worker-llm'), 'cpu');
  writeFileSync(join(src, 'llama.dll'), 'llama');
  writeFileSync(join(src, 'ggml.dll'), 'ggml');
  writeFileSync(join(toolkit, 'bin', 'cudart64_12.dll'), 'cudart');
  const { accel } = stageLlmPayloadFromDir({
    srcDir: src,
    runtimeRoots: [resources],
    cudaRoot: toolkit,
  });
  assert.equal(accel, 'cpu');
  assert.equal(existsSync(join(llmPayloadDir(resources, 'cpu'), 'cudart64_12.dll')), false);
  rmSync(root, { recursive: true, force: true });
});

test('restaging cpu prunes stale cuda libs in the cpu dir only', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-prune-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  mkdirSync(src);
  writeFileSync(join(src, 'aifs-worker-llm'), 'cpu');
  writeFileSync(join(src, 'llama.dll'), 'llama');
  writeFileSync(join(src, 'ggml.dll'), 'ggml');
  const cpuDest = llmPayloadDir(resources, 'cpu');
  mkdirSync(cpuDest, { recursive: true });
  writeFileSync(join(cpuDest, 'ggml-cuda.dll'), 'stale');
  const cudaDest = llmPayloadDir(resources, 'cuda');
  mkdirSync(cudaDest, { recursive: true });
  writeFileSync(join(cudaDest, 'ggml-cuda.dll'), 'keep');
  stageLlmPayloadFromDir({ srcDir: src, runtimeRoots: [resources] });
  assert.equal(existsSync(join(cpuDest, 'ggml-cuda.dll')), false);
  assert.equal(readFileSync(join(cudaDest, 'ggml-cuda.dll'), 'utf8'), 'keep');
  rmSync(root, { recursive: true, force: true });
});

test('symlink runtime libs are staged', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-stage-llm-link-'));
  const src = join(root, 'src');
  const resources = join(root, 'resources');
  mkdirSync(src);
  writeFileSync(join(src, 'aifs-worker-llm'), 'worker');
  writeFileSync(join(src, 'libllama.so.0'), 'llama');
  writeFileSync(join(src, 'libggml.so.0'), 'ggml');
  symlinkSync('libllama.so.0', join(src, 'libllama.so'));
  const { accel } = stageLlmPayloadFromDir({ srcDir: src, runtimeRoots: [resources] });
  assert.equal(accel, 'cpu');
  assert.equal(existsSync(join(llmPayloadDir(resources, 'cpu'), 'libllama.so')), true);
  rmSync(root, { recursive: true, force: true });
});

test('mocked node -e cargo engine-llm must not stage', () => {
  assert.equal(shouldStageAfterEngineLlm('cargo', ['cargo', 'engine-llm']), true);
  assert.equal(
    shouldStageAfterEngineLlm('/usr/bin/node', ['-e', '0', 'cargo', 'engine-llm']),
    false,
  );
});
