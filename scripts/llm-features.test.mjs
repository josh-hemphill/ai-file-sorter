import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  appendEngineLlmFeatures,
  applyLinuxLlamaCxx,
  argvHasFeature,
  isCargoEngineLlm,
  parseLlmFeatures,
} from './llm-features.mjs';

const here = dirname(fileURLToPath(import.meta.url));

test('parseLlmFeatures canonicalizes aliases and drops junk', () => {
  assert.deepEqual(parseLlmFeatures('cuda'), ['cuda']);
  assert.deepEqual(parseLlmFeatures('vulcan,CUDA'), ['vulkan', 'cuda']);
  assert.deepEqual(parseLlmFeatures('vulkan, metal, llama'), ['vulkan', 'metal']);
  assert.deepEqual(parseLlmFeatures('nope'), []);
  assert.deepEqual(parseLlmFeatures(''), []);
});

test('isCargoEngineLlm matches the llama worker alias and package', () => {
  assert.equal(isCargoEngineLlm(['cargo', 'engine-llm']), true);
  assert.equal(
    isCargoEngineLlm(['cargo', 'build', '-p', 'aifs-worker-llm', '--features', 'llama']),
    true,
  );
  assert.equal(isCargoEngineLlm(['cargo', 'engine-bins']), false);
  assert.equal(isCargoEngineLlm(['node', '-e', '0']), false);
});

test('appendEngineLlmFeatures adds missing CUDA/Vulkan from AIFS_LLM_FEATURES', () => {
  assert.deepEqual(appendEngineLlmFeatures(['cargo', 'engine-llm'], {}), ['cargo', 'engine-llm']);
  assert.deepEqual(
    appendEngineLlmFeatures(['cargo', 'engine-llm'], { AIFS_LLM_FEATURES: 'cuda' }),
    ['cargo', 'engine-llm', '--features', 'llama,cuda'],
  );
  assert.deepEqual(
    appendEngineLlmFeatures(
      ['cargo', 'engine-llm', '--manifest-path', '../../Cargo.toml'],
      { AIFS_LLM_FEATURES: 'vulkan' },
    ),
    [
      'cargo',
      'engine-llm',
      '--manifest-path',
      '../../Cargo.toml',
      '--features',
      'llama,vulkan',
    ],
  );
  assert.deepEqual(
    appendEngineLlmFeatures(
      ['cargo', 'engine-llm', '--features', 'llama,cuda'],
      { AIFS_LLM_FEATURES: 'cuda' },
    ),
    ['cargo', 'engine-llm', '--features', 'llama,cuda'],
  );
  assert.deepEqual(
    appendEngineLlmFeatures(
      ['cargo', 'engine-llm', '--features', 'llama,cuda'],
      { AIFS_LLM_FEATURES: 'cuda,vulkan' },
    ),
    ['cargo', 'engine-llm', '--features', 'llama,cuda', '--features', 'llama,vulkan'],
  );
});

test('argvHasFeature reads cargo -F lists', () => {
  assert.equal(argvHasFeature(['--features', 'llama,cuda'], 'cuda'), true);
  assert.equal(argvHasFeature(['-F', 'vulkan'], 'cuda'), false);
  assert.equal(argvHasFeature(['--features=metal'], 'metal'), true);
});

test('applyLinuxLlamaCxx sets g++ only for Linux llama worker builds', () => {
  const env = {};
  assert.equal(
    applyLinuxLlamaCxx({ argv: ['cargo', 'engine-llm'], env, platform: 'linux' }),
    'g++',
  );
  assert.equal(env.CXX, 'g++');
  const preset = { CXX: 'clang++' };
  assert.equal(
    applyLinuxLlamaCxx({ argv: ['cargo', 'engine-llm'], env: preset, platform: 'linux' }),
    undefined,
  );
  assert.equal(preset.CXX, 'clang++');
  const win = {};
  assert.equal(
    applyLinuxLlamaCxx({ argv: ['cargo', 'engine-llm'], env: win, platform: 'win32' }),
    undefined,
  );
});

test('with-cmake-generator.mjs forwards AIFS_LLM_FEATURES onto cargo engine-llm', () => {
  const env = { ...process.env, AIFS_LLM_FEATURES: 'cuda' };
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      process.execPath,
      '-e',
      'process.exit(process.argv.slice(1).includes("llama,cuda") ? 0 : 1)',
      'cargo',
      'engine-llm',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0, result.stderr);
});

test('with-cmake-generator.mjs --packaged still forwards AIFS_LLM_FEATURES', () => {
  const env = { ...process.env, AIFS_LLM_FEATURES: 'vulkan' };
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      '--packaged',
      process.execPath,
      '-e',
      'process.exit(process.argv.slice(1).includes("llama,vulkan") ? 0 : 1)',
      'cargo',
      'engine-llm',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0, result.stderr);
});

test('with-cmake-generator.mjs leaves CPU llama when AIFS_LLM_FEATURES is unset', () => {
  const env = { ...process.env };
  delete env.AIFS_LLM_FEATURES;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-cmake-generator.mjs'),
      process.execPath,
      '-e',
      'process.exit(process.argv.slice(1).includes("--features") ? 1 : 0)',
      'cargo',
      'engine-llm',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0, result.stderr);
});

test('with-llm-features.mjs sets AIFS_LLM_FEATURES for the child', () => {
  const env = { ...process.env };
  delete env.AIFS_LLM_FEATURES;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-llm-features.mjs'),
      'vulcan',
      process.execPath,
      '-e',
      'process.exit(process.env.AIFS_LLM_FEATURES === "vulkan" ? 0 : 1)',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0, result.stderr);
});

test('with-llm-features.mjs accepts comma-separated aliases', () => {
  const env = { ...process.env };
  delete env.AIFS_LLM_FEATURES;
  const result = spawnSync(
    process.execPath,
    [
      join(here, 'with-llm-features.mjs'),
      'cuda,vulcan',
      process.execPath,
      '-e',
      'process.exit(process.env.AIFS_LLM_FEATURES === "cuda,vulkan" ? 0 : 1)',
    ],
    { encoding: 'utf8', env },
  );
  assert.equal(result.status, 0, result.stderr);
});

test('with-llm-features.mjs rejects empty or unknown features', () => {
  const missing = spawnSync(process.execPath, [join(here, 'with-llm-features.mjs')], {
    encoding: 'utf8',
  });
  assert.equal(missing.status, 2);
  const bad = spawnSync(
    process.execPath,
    [join(here, 'with-llm-features.mjs'), 'nope', process.execPath, '-e', '0'],
    { encoding: 'utf8' },
  );
  assert.equal(bad.status, 2);
  assert.match(bad.stderr, /unknown LLM features/);
});
