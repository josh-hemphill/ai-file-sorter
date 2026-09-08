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
  assert.equal(pkg.scripts.llama, 'cargo engine-llm');
  assert.equal(pkg.scripts.cli, 'cargo aifs');
  assert.match(pkg.scripts.desktop, /pnpm llama/);
  assert.match(pkg.scripts.desktop, /--filter desktop tauri dev/);
  assert.match(pkg.scripts['test:desktop'], /--filter desktop test/);
});
