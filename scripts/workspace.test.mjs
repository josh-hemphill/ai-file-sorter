import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

test('root pnpm scripts orchestrate cargo and the desktop package', async () => {
  const pkg = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
  const workspace = await readFile(join(root, 'pnpm-workspace.yaml'), 'utf8');
  assert.equal(pkg.private, true);
  assert.match(workspace, /apps\/desktop/);
  assert.equal(pkg.scripts.build, 'cargo engine-bins');
  assert.equal(
    pkg.scripts.llama,
    'node scripts/with-cmake-generator.mjs cargo engine-llm',
  );
  assert.equal(
    pkg.scripts['llama:cuda'],
    'node scripts/with-cmake-generator.mjs cargo engine-llm --features llama,cuda',
  );
  assert.equal(pkg.scripts.cli, 'cargo aifs');
  assert.equal(pkg.scripts.test, 'pnpm build && cargo test --workspace');
  assert.equal(pkg.scripts.check, 'pnpm fmt:check && pnpm clippy && pnpm test');
  assert.equal(
    pkg.scripts.desktop,
    'pnpm build && pnpm llama && pnpm --filter desktop tauri dev',
  );
  assert.equal(
    pkg.scripts['test:desktop'],
    'node --test scripts/workspace.test.mjs scripts/windows-cmake-generator.test.mjs scripts/llama-cuda-env.test.mjs scripts/packaged-ggml.test.mjs && pnpm --filter desktop test && pnpm --filter desktop exec vue-tsc --noEmit',
  );
});
