import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import {
  inferAccelFromLibNames,
  isWorkerRuntimeLib,
  llmPayloadDir,
  stageLlmPayloadFromDir,
} from './stage-llm-payload.mjs';

test('runtime lib matcher copies ggml plugins and skips nvcuda', () => {
  assert.equal(isWorkerRuntimeLib('ggml.dll'), true);
  assert.equal(isWorkerRuntimeLib('ggml-cuda.dll'), true);
  assert.equal(isWorkerRuntimeLib('libllama.so.0'), true);
  assert.equal(isWorkerRuntimeLib('cudart64_12.dll'), true);
  assert.equal(isWorkerRuntimeLib('nvcuda.dll'), false);
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
  const accel = stageLlmPayloadFromDir({
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
  const accel = stageLlmPayloadFromDir({
    srcDir: src,
    runtimeRoots: [resources],
    cudaRoot: toolkit,
  });
  assert.equal(accel, 'cuda');
  const dest = llmPayloadDir(resources, 'cuda');
  assert.equal(readFileSync(join(dest, 'ggml-cuda.dll'), 'utf8'), 'cuda');
  assert.equal(readFileSync(join(dest, 'cudart64_12.dll'), 'utf8'), 'cudart');
  assert.equal(readFileSync(join(dest, 'aifs-worker-llm'), 'utf8'), 'cuda-worker');
  assert.equal(existsSync(join(dest, 'nvcuda.dll')), false);
  rmSync(root, { recursive: true, force: true });
});
