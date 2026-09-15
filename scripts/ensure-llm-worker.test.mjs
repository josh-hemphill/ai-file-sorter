import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { llmPayloadDir } from './stage-llm-payload.mjs';
import { skipEngineLlmReason } from './ensure-llm-worker.mjs';

function tempRoot() {
  return mkdtempSync(join(tmpdir(), 'aifs-ensure-llm-'));
}

test('skipEngineLlmReason honors AIFS_SKIP_LLAMA and AIFS_FORCE_LLAMA', () => {
  const root = tempRoot();
  assert.equal(
    skipEngineLlmReason({ env: { AIFS_SKIP_LLAMA: '1' }, runtimeRoots: [root] }),
    'AIFS_SKIP_LLAMA is set',
  );
  assert.equal(
    skipEngineLlmReason({
      env: { AIFS_SKIP_LLAMA: '1', AIFS_FORCE_LLAMA: '1' },
      runtimeRoots: [root],
    }),
    null,
  );
  rmSync(root, { recursive: true, force: true });
});

test('skipEngineLlmReason reuses a complete CUDA payload', () => {
  const root = tempRoot();
  const dir = llmPayloadDir(root, 'cuda');
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'aifs-worker-llm.exe'), Buffer.from('MZ\0cublas64_13.dll\0'));
  writeFileSync(join(dir, 'cublas64_13.dll'), 'cublas');
  assert.match(
    skipEngineLlmReason({
      env: { AIFS_LLM_FEATURES: 'cuda' },
      runtimeRoots: [root],
    }),
    /llm-runtime\/cuda already complete/,
  );
  assert.equal(
    skipEngineLlmReason({ env: {}, runtimeRoots: [root] }),
    null,
    'CPU launch still compiles when only cuda is staged',
  );
  rmSync(root, { recursive: true, force: true });
});

test('skipEngineLlmReason compiles when the staged CUDA worker is a stub', () => {
  const root = tempRoot();
  const dir = llmPayloadDir(root, 'cuda');
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'aifs-worker-llm.exe'), 'stub infer worker');
  writeFileSync(join(dir, 'cublas64_13.dll'), 'cublas');
  assert.equal(
    skipEngineLlmReason({
      env: { AIFS_LLM_FEATURES: 'cuda' },
      runtimeRoots: [root],
    }),
    null,
  );
  rmSync(root, { recursive: true, force: true });
});
