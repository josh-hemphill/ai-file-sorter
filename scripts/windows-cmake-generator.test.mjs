import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  VS_2019,
  VS_2022,
  VS_2026,
  applyWindowsCmakeGenerator,
  cmakeGeneratorForVsVersion,
  clearLlamaCppSysCmakeCache,
  parseCmakeCapabilitiesGenerators,
  parseCmakeHelpGenerators,
  parseCmdSetOutput,
  pickWindowsCmakeGenerator,
  planWindowsLlamaBuild,
} from './windows-cmake-generator.mjs';

const here = dirname(fileURLToPath(import.meta.url));

const CMAKE_HELP_WITHOUT_VS_2026 = `
Generators

The following generators are available on this platform (* marks default):
* Visual Studio 17 2022        = Generates Visual Studio 2022 project files.
  Visual Studio 16 2019        = Generates Visual Studio 2019 project files.
  Visual Studio 15 2017        = Generates Visual Studio 2017 project files.
  NMake Makefiles              = Generates NMake makefiles.
  Ninja                        = Generates build.ninja files.
  Ninja Multi-Config           = Generates build-<Config>.ninja files.
`;

const CMAKE_HELP_WITH_VS_2026 = `
Generators

The following generators are available on this platform (* marks default):
* Visual Studio 18 2026        = Generates Visual Studio 2026 project files.
  Visual Studio 17 2022        = Generates Visual Studio 2022 project files.
  Ninja                        = Generates build.ninja files.
`;

test('parseCmakeHelpGenerators reads starred and unstarred Visual Studio names', () => {
  const names = parseCmakeHelpGenerators(CMAKE_HELP_WITHOUT_VS_2026);
  assert.deepEqual(names, [
    VS_2022,
    VS_2019,
    'Visual Studio 15 2017',
    'NMake Makefiles',
    'Ninja',
    'Ninja Multi-Config',
  ]);
  assert.equal(names.includes(VS_2026), false);
  assert.ok(parseCmakeHelpGenerators(CMAKE_HELP_WITH_VS_2026).includes(VS_2026));
});

test('parseCmakeCapabilitiesGenerators reads cmake -E capabilities', () => {
  const names = parseCmakeCapabilitiesGenerators(
    JSON.stringify({
      generators: [{ name: VS_2022 }, { name: 'Ninja' }, { extraGenerators: [] }],
    }),
  );
  assert.deepEqual(names, [VS_2022, 'Ninja']);
});

test('cmakeGeneratorForVsVersion matches cmake-rs Visual Studio names', () => {
  assert.equal(cmakeGeneratorForVsVersion(18), VS_2026);
  assert.equal(cmakeGeneratorForVsVersion(17), VS_2022);
  assert.equal(cmakeGeneratorForVsVersion(16), VS_2019);
  assert.equal(cmakeGeneratorForVsVersion(14), undefined);
});

test('parseCmdSetOutput reads cmd /c set lines', () => {
  const env = parseCmdSetOutput('INCLUDE=C:\\inc\r\nPath=C:\\ninja;C:\\old\r\nNOT A VAR\r\nLIB=C:\\lib\r\n');
  assert.equal(env.INCLUDE, 'C:\\inc');
  assert.equal(env.Path, 'C:\\ninja;C:\\old');
  assert.equal(env.LIB, 'C:\\lib');
  assert.equal(env['NOT A VAR'], undefined);
});

test('pickWindowsCmakeGenerator leaves Unix and explicit CMAKE_GENERATOR alone', () => {
  const listed = parseCmakeHelpGenerators(CMAKE_HELP_WITHOUT_VS_2026);
  assert.equal(
    pickWindowsCmakeGenerator({
      platform: 'linux',
      listedGenerators: listed,
      latestVsMajor: 18,
      installedVsMajors: [18],
      hasNinja: true,
    }),
    undefined,
  );
  assert.equal(
    pickWindowsCmakeGenerator({
      platform: 'win32',
      existingGenerator: 'Ninja',
      listedGenerators: listed,
      latestVsMajor: 18,
      installedVsMajors: [18],
      hasNinja: true,
    }),
    undefined,
  );
});

test('pickWindowsCmakeGenerator does nothing when CMake lists Visual Studio 18 2026', () => {
  assert.equal(
    pickWindowsCmakeGenerator({
      platform: 'win32',
      listedGenerators: parseCmakeHelpGenerators(CMAKE_HELP_WITH_VS_2026),
      latestVsMajor: 18,
      installedVsMajors: [18],
      hasNinja: false,
    }),
    undefined,
  );
});

test('pickWindowsCmakeGenerator prefers VS 2022 when cmake-rs would pick unknown VS 2026', () => {
  const listed = parseCmakeHelpGenerators(CMAKE_HELP_WITHOUT_VS_2026);
  assert.equal(
    pickWindowsCmakeGenerator({
      platform: 'win32',
      listedGenerators: listed,
      latestVsMajor: 18,
      installedVsMajors: [18, 17],
      hasNinja: true,
    }),
    VS_2022,
  );
});

test('pickWindowsCmakeGenerator uses Ninja when only VS 2026 is installed', () => {
  const listed = parseCmakeHelpGenerators(CMAKE_HELP_WITHOUT_VS_2026);
  assert.equal(
    pickWindowsCmakeGenerator({
      platform: 'win32',
      listedGenerators: listed,
      latestVsMajor: 18,
      installedVsMajors: [18],
      hasNinja: true,
    }),
    'Ninja',
  );
});

test('pickWindowsCmakeGenerator does not force VS 2022 when it is not installed', () => {
  const listed = parseCmakeHelpGenerators(CMAKE_HELP_WITHOUT_VS_2026);
  assert.equal(
    pickWindowsCmakeGenerator({
      platform: 'win32',
      listedGenerators: listed,
      latestVsMajor: 18,
      installedVsMajors: [18],
      hasNinja: false,
    }),
    undefined,
  );
  const plan = planWindowsLlamaBuild({
    platform: 'win32',
    listedGenerators: listed,
    latestVsMajor: 18,
    installedVsMajors: [18],
    hasNinja: false,
  });
  assert.equal(plan.kind, 'error');
  assert.match(plan.message, /winget install Ninja-build.Ninja/);
  assert.match(plan.message, /llama-cpp-sys-2/);
});

test('applyWindowsCmakeGenerator writes CMAKE_GENERATOR and is a no-op on Unix', () => {
  const env = {};
  const messages = [];
  const chosen = applyWindowsCmakeGenerator({
    env,
    platform: 'win32',
    listGenerators: () => parseCmakeHelpGenerators(CMAKE_HELP_WITHOUT_VS_2026),
    inspectBuildTools: () => ({
      latestVsMajor: 18,
      installedVsMajors: [18, 17],
      hasNinja: false,
    }),
    clearCache: () => 0,
    log: (message) => messages.push(message),
  });
  assert.equal(chosen.kind, 'set');
  assert.equal(chosen.generator, VS_2022);
  assert.equal(env.CMAKE_GENERATOR, VS_2022);
  assert.match(messages[0], /CMAKE_GENERATOR=Visual Studio 17 2022/);

  const unixEnv = {};
  assert.equal(
    applyWindowsCmakeGenerator({
      env: unixEnv,
      platform: 'linux',
      listGenerators: () => {
        throw new Error('cmake must not run on Unix');
      },
      inspectBuildTools: () => {
        throw new Error('vswhere must not run on Unix');
      },
    }).kind,
    'noop',
  );
  assert.equal(unixEnv.CMAKE_GENERATOR, undefined);

  const preset = { CMAKE_GENERATOR: 'Ninja' };
  const kept = applyWindowsCmakeGenerator({
    env: preset,
    platform: 'win32',
    listGenerators: () => {
      throw new Error('cmake must not run when CMAKE_GENERATOR is set');
    },
    inspectBuildTools: () => ({ hasNinja: true, vsInstallPath: 'C:\\VS' }),
    loadMsvc: () => true,
    log: () => {},
  });
  assert.equal(kept.kind, 'keep');
  assert.equal(preset.CMAKE_GENERATOR, 'Ninja');
});

test('applyWindowsCmakeGenerator prepends bundled Ninja and loads MSVC env', () => {
  const env = { PATH: 'C:\\old' };
  const loaded = [];
  const caches = [];
  const messages = [];
  const plan = applyWindowsCmakeGenerator({
    env,
    platform: 'win32',
    listGenerators: () => parseCmakeHelpGenerators(CMAKE_HELP_WITHOUT_VS_2026),
    inspectBuildTools: () => ({
      latestVsMajor: 18,
      installedVsMajors: [18],
      hasNinja: true,
      ninjaPath: 'C:\\VS\\Common7\\IDE\\CommonExtensions\\Microsoft\\CMake\\Ninja\\ninja.exe',
      vsInstallPath: 'C:\\VS',
    }),
    loadMsvc: (target, path) => {
      loaded.push(path);
      target.INCLUDE = 'C:\\inc';
      return true;
    },
    clearCache: (root) => caches.push(root),
    repoRoot: '/repo',
    log: (message) => messages.push(message),
  });
  assert.equal(plan.kind, 'set');
  assert.equal(env.CMAKE_GENERATOR, 'Ninja');
  assert.equal(
    env.PATH,
    'C:\\VS\\Common7\\IDE\\CommonExtensions\\Microsoft\\CMake\\Ninja;C:\\old',
  );
  assert.deepEqual(loaded, ['C:\\VS']);
  assert.deepEqual(caches, ['/repo']);
  assert.equal(env.INCLUDE, 'C:\\inc');
  assert.match(messages[0], /CMAKE_GENERATOR=Ninja/);
});

test('applyWindowsCmakeGenerator fails closed when VS 2026 has no Ninja or VS 2022', () => {
  const messages = [];
  const plan = applyWindowsCmakeGenerator({
    env: {},
    platform: 'win32',
    listGenerators: () => parseCmakeHelpGenerators(CMAKE_HELP_WITHOUT_VS_2026),
    inspectBuildTools: () => ({
      latestVsMajor: 18,
      installedVsMajors: [18],
      hasNinja: false,
    }),
    log: (message) => messages.push(message),
  });
  assert.equal(plan.kind, 'error');
  assert.match(messages[0], /CMake 4.2/);
});

test('clearLlamaCppSysCmakeCache removes CMakeCache.txt under llama-cpp-sys-2', () => {
  const root = mkdtempSync(join(tmpdir(), 'aifs-cmake-'));
  const cacheDir = join(root, 'target', 'debug', 'build', 'llama-cpp-sys-2-abc', 'out', 'build');
  mkdirSync(cacheDir, { recursive: true });
  writeFileSync(join(cacheDir, 'CMakeCache.txt'), 'CMAKE_GENERATOR:INTERNAL=Visual Studio 17 2022\n');
  const other = join(root, 'target', 'debug', 'build', 'other-crate-xyz', 'out', 'build');
  mkdirSync(other, { recursive: true });
  writeFileSync(join(other, 'CMakeCache.txt'), 'keep\n');
  const logs = [];
  assert.equal(clearLlamaCppSysCmakeCache(root, (message) => logs.push(message)), 1);
  assert.equal(existsSync(join(cacheDir, 'CMakeCache.txt')), false);
  assert.equal(existsSync(join(other, 'CMakeCache.txt')), true);
  assert.match(logs[0], /cleared 1/);
});

test('with-cmake-generator.mjs forwards the child exit code', () => {
  const result = spawnSync(
    process.execPath,
    [join(here, 'with-cmake-generator.mjs'), process.execPath, '-e', 'process.exit(7)'],
    { encoding: 'utf8' },
  );
  assert.equal(result.status, 7);
  assert.equal(result.stderr.includes('DEP0190'), false);
});

test('with-cmake-generator.mjs does not set CMAKE_GENERATOR on Unix', () => {
  if (process.platform === 'win32') return;
  const env = { ...process.env };
  delete env.CMAKE_GENERATOR;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      process.execPath,
      '-e',
      'process.exit(process.env.CMAKE_GENERATOR ? 1 : 0)',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0);
});

test('with-cmake-generator.mjs requires a command', () => {
  const result = spawnSync(process.execPath, [join(here, 'with-cmake-generator.mjs')], {
    encoding: 'utf8',
  });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /usage:/);
});
