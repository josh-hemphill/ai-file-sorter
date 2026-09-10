import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  MACOS_GGML_CMAKE_ENV,
  MACOS_LOADER_RPATH_RUSTFLAG,
  PORTABLE_SSE42_RUSTFLAG,
  PORTABLE_X86_GGML_ENV,
  appendRustflagChunk,
  applyPackagedGgmlEnv,
  packagedGgmlCmakeEnv,
  withoutTargetCpuNative,
} from './packaged-ggml.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');

function cleanChildEnv() {
  const env = { ...process.env };
  delete env.CMAKE_ARGS;
  delete env.CMAKE_INSTALL_RPATH;
  delete env.CMAKE_IGNORE_PREFIX_PATH;
  delete env.CMAKE_MACOSX_RPATH;
  delete env.CMAKE_BUILD_WITH_INSTALL_RPATH;
  delete env.RUSTFLAGS;
  for (const key of Object.keys(PORTABLE_X86_GGML_ENV)) {
    delete env[key];
  }
  return env;
}

function assertPortableX86GgmlEnv(actual) {
  assert.deepEqual(
    Object.fromEntries(Object.keys(PORTABLE_X86_GGML_ENV).map((key) => [key, actual[key]])),
    PORTABLE_X86_GGML_ENV,
  );
}

test('packagedGgmlCmakeEnv is a no-op for local llama builds', () => {
  assert.deepEqual(packagedGgmlCmakeEnv({ platform: 'linux', packaged: false }), {});
  assert.deepEqual(packagedGgmlCmakeEnv({ platform: 'win32', packaged: false }), {});
  assert.deepEqual(packagedGgmlCmakeEnv({ platform: 'darwin', packaged: false }), {});
});

test('packaged Windows and Linux pin SSE4.2 and force GGML AVX* off', () => {
  const env = { RUSTFLAGS: '-C target-cpu=native -D warnings' };
  const applied = applyPackagedGgmlEnv({ env, platform: 'linux', packaged: true });
  assert.equal(applied.RUSTFLAGS.includes('target-cpu=native'), false);
  assert.equal(applied.RUSTFLAGS.includes(PORTABLE_SSE42_RUSTFLAG), true);
  assert.equal(env.CMAKE_INSTALL_RPATH, undefined);
  assertPortableX86GgmlEnv(applied);
  assertPortableX86GgmlEnv(env);
  const win = applyPackagedGgmlEnv({
    env: { RUSTFLAGS: '-Ctarget-cpu=native' },
    platform: 'win32',
    packaged: true,
  });
  assert.equal(win.RUSTFLAGS.includes(PORTABLE_SSE42_RUSTFLAG), true);
  assert.equal(win.RUSTFLAGS.includes('target-cpu=native'), false);
  assertPortableX86GgmlEnv(win);
});

test('packaged macOS sets CMAKE_* env names and rpath RUSTFLAGS', () => {
  const env = {};
  const applied = applyPackagedGgmlEnv({ env, platform: 'darwin', packaged: true });
  assert.deepEqual(packagedGgmlCmakeEnv({ platform: 'darwin', packaged: true }), MACOS_GGML_CMAKE_ENV);
  assert.equal(env.CMAKE_MACOSX_RPATH, MACOS_GGML_CMAKE_ENV.CMAKE_MACOSX_RPATH);
  assert.equal(env.CMAKE_BUILD_WITH_INSTALL_RPATH, 'ON');
  assert.equal(applied.CMAKE_INSTALL_RPATH, '@loader_path');
  assert.equal(applied.CMAKE_IGNORE_PREFIX_PATH, '/opt/homebrew;/usr/local');
  assert.match(applied.CMAKE_IGNORE_PREFIX_PATH, /\/opt\/homebrew/);
  assert.match(applied.CMAKE_IGNORE_PREFIX_PATH, /\/usr\/local/);
  assert.equal(applied.RUSTFLAGS.includes(MACOS_LOADER_RPATH_RUSTFLAG), true);
  assert.equal(applied.RUSTFLAGS.includes(PORTABLE_SSE42_RUSTFLAG), false);
  assert.equal(applied.GGML_AVX2, undefined);
});

test('withoutTargetCpuNative and appendRustflagChunk keep chunks intact', () => {
  assert.equal(withoutTargetCpuNative('-C target-cpu=native'), '');
  assert.equal(withoutTargetCpuNative('-Ctarget-cpu=native'), '');
  assert.equal(
    withoutTargetCpuNative('-C target-cpu=native -D warnings'),
    '-D warnings',
  );
  assert.equal(
    appendRustflagChunk(MACOS_LOADER_RPATH_RUSTFLAG, MACOS_LOADER_RPATH_RUSTFLAG),
    MACOS_LOADER_RPATH_RUSTFLAG,
  );
});

test('applyPackagedGgmlEnv leaves process env alone when not packaging', () => {
  const env = { CMAKE_INSTALL_RPATH: 'keep-me', RUSTFLAGS: '-D warnings', GGML_AVX2: 'keep-me' };
  assert.equal(
    applyPackagedGgmlEnv({ env, platform: 'linux', packaged: false }),
    undefined,
  );
  assert.equal(env.CMAKE_INSTALL_RPATH, 'keep-me');
  assert.equal(env.RUSTFLAGS, '-D warnings');
  assert.equal(env.GGML_AVX2, 'keep-me');
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

test('with-cmake-generator.mjs --packaged sets portable GGML_* on Linux', () => {
  if (process.platform === 'darwin' || process.platform === 'win32') return;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      '--packaged',
      process.execPath,
      '-e',
      [
        'const ok =',
        '  process.env.RUSTFLAGS?.includes("-C target-feature=+sse4.2") &&',
        '  !process.env.RUSTFLAGS.includes("target-cpu=native") &&',
        '  process.env.GGML_AVX2 === "OFF" &&',
        '  process.env.GGML_AVX === "OFF" &&',
        '  process.env.GGML_FMA === "OFF" &&',
        '  process.env.GGML_SSE42 === "ON" &&',
        '  !process.env.CMAKE_ARGS;',
        'process.exit(ok ? 0 : 1);',
      ].join(''),
    ],
    {
      encoding: 'utf8',
      env: { ...cleanChildEnv(), RUSTFLAGS: '-C target-cpu=native -D warnings' },
    },
  );
  assert.equal(result.status, 0, result.stderr);
});

test('with-cmake-generator.mjs without --packaged leaves CMAKE_INSTALL_RPATH unset', () => {
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      process.execPath,
      '-e',
      'process.exit(process.env.CMAKE_INSTALL_RPATH || process.env.GGML_AVX2 ? 1 : 0)',
    ],
    { encoding: 'utf8', env: cleanChildEnv() },
  );
  assert.equal(result.status, 0, result.stderr);
});
