import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  GGML_NATIVE_OFF,
  MACOS_GGML_CMAKE_ARGS,
  MACOS_LOADER_RPATH_RUSTFLAG,
  applyPackagedGgmlEnv,
  mergeFlagList,
  packagedGgmlCmakeFlags,
} from './packaged-ggml.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');

test('packagedGgmlCmakeFlags is a no-op for local llama builds', () => {
  assert.deepEqual(packagedGgmlCmakeFlags({ platform: 'linux', packaged: false }), []);
  assert.deepEqual(packagedGgmlCmakeFlags({ platform: 'win32', packaged: false }), []);
  assert.deepEqual(packagedGgmlCmakeFlags({ platform: 'darwin', packaged: false }), []);
});

test('packaged Windows and Linux CPU wheels disable GGML_NATIVE', () => {
  assert.deepEqual(packagedGgmlCmakeFlags({ platform: 'linux', packaged: true }), [
    GGML_NATIVE_OFF,
  ]);
  assert.deepEqual(packagedGgmlCmakeFlags({ platform: 'win32', packaged: true }), [
    GGML_NATIVE_OFF,
  ]);
});

test('packaged macOS rpaths ggml and ignores Homebrew prefixes', () => {
  const flags = packagedGgmlCmakeFlags({ platform: 'darwin', packaged: true });
  assert.deepEqual(flags, MACOS_GGML_CMAKE_ARGS);
  assert.equal(flags.includes(GGML_NATIVE_OFF), false);
  assert.match(flags.join(' '), /@loader_path/);
  assert.match(flags.join(' '), /\/opt\/homebrew/);
});

test('mergeFlagList does not duplicate existing CMAKE_ARGS', () => {
  assert.deepEqual(mergeFlagList('-DGGML_NATIVE=OFF', [GGML_NATIVE_OFF]), [
    GGML_NATIVE_OFF,
  ]);
  assert.deepEqual(mergeFlagList('', [GGML_NATIVE_OFF]), [GGML_NATIVE_OFF]);
});

test('applyPackagedGgmlEnv leaves process env alone when not packaging', () => {
  const env = { CMAKE_ARGS: '-DGGML_CUDA=ON' };
  assert.equal(
    applyPackagedGgmlEnv({ env, platform: 'linux', packaged: false }),
    undefined,
  );
  assert.equal(env.CMAKE_ARGS, '-DGGML_CUDA=ON');
});

test('applyPackagedGgmlEnv sets portable CMAKE_ARGS on Linux package builds', () => {
  const env = { CMAKE_ARGS: '-DGGML_CUDA=ON' };
  const applied = applyPackagedGgmlEnv({ env, platform: 'linux', packaged: true });
  assert.match(applied.CMAKE_ARGS, /GGML_CUDA=ON/);
  assert.match(applied.CMAKE_ARGS, /GGML_NATIVE=OFF/);
  assert.equal(env.RUSTFLAGS, undefined);
});

test('applyPackagedGgmlEnv sets macOS rpath on CMAKE_ARGS and RUSTFLAGS', () => {
  const env = {};
  const applied = applyPackagedGgmlEnv({ env, platform: 'darwin', packaged: true });
  for (const flag of MACOS_GGML_CMAKE_ARGS) {
    assert.equal(applied.CMAKE_ARGS.includes(flag), true, flag);
  }
  assert.equal(applied.RUSTFLAGS.includes(MACOS_LOADER_RPATH_RUSTFLAG), true);
});

test('tauri beforeBuild uses --packaged; beforeDev and pnpm llama do not', async () => {
  const conf = JSON.parse(
    await readFile(join(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'),
  );
  assert.match(conf.build.beforeBuildCommand, /with-cmake-generator\.mjs --packaged/);
  assert.doesNotMatch(conf.build.beforeDevCommand, /--packaged/);
  const pkg = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
  assert.equal(pkg.scripts.llama.includes('--packaged'), false);
});

test('with-cmake-generator.mjs --packaged sets GGML_NATIVE=OFF on Linux', () => {
  if (process.platform === 'darwin' || process.platform === 'win32') return;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      '--packaged',
      process.execPath,
      '-e',
      'process.exit(process.env.CMAKE_ARGS?.includes("-DGGML_NATIVE=OFF") ? 0 : 1)',
    ],
    { encoding: 'utf8' },
  );
  assert.equal(result.status, 0, result.stderr);
});

test('with-cmake-generator.mjs without --packaged leaves CMAKE_ARGS unset', () => {
  const env = { ...process.env };
  delete env.CMAKE_ARGS;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      process.execPath,
      '-e',
      'process.exit(process.env.CMAKE_ARGS ? 1 : 0)',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0, result.stderr);
});
