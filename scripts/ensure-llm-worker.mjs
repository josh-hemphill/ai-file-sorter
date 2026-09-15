#!/usr/bin/env node
/**
 * Runs `cargo engine-llm` unless a complete payload is already staged
 * (`AIFS_SKIP_LLAMA` always skips; `AIFS_FORCE_LLAMA` always rebuilds).
 */
import { spawn } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expectedAccelFromEnv, llmPayloadDir, payloadDirComplete } from './stage-llm-payload.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const wrap = join(here, 'with-cmake-generator.mjs');

function truthy(value) {
  return ['1', 'true', 'yes', 'on'].includes(String(value ?? '').trim().toLowerCase());
}

function profileFromArgv(argv) {
  return argv.includes('--release') ? 'release' : 'debug';
}

/** `target/{debug,release}` plus Tauri `resources/` for the given argv. */
export function defaultRuntimeRoots({
  argv = process.argv.slice(2),
  cwd = join(here, '..'),
} = {}) {
  const profile = profileFromArgv(argv);
  return [
    join(cwd, 'target', profile),
    join(cwd, 'apps/desktop/src-tauri/resources'),
  ];
}

/** Why `cargo engine-llm` can be skipped, or `null` to compile. */
export function skipEngineLlmReason({
  env = process.env,
  runtimeRoots = defaultRuntimeRoots(),
} = {}) {
  if (truthy(env.AIFS_FORCE_LLAMA)) {
    return null;
  }
  if (truthy(env.AIFS_SKIP_LLAMA)) {
    return 'AIFS_SKIP_LLAMA is set';
  }
  const accel = expectedAccelFromEnv(env) ?? 'cpu';
  for (const root of runtimeRoots) {
    const dir = llmPayloadDir(root, accel);
    if (payloadDirComplete(dir, accel)) {
      return `llm-runtime/${accel} already complete at ${dir}`;
    }
  }
  return null;
}

const invokedDirectly =
  process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];

if (invokedDirectly) {
  const argv = process.argv.slice(2);
  const reason = skipEngineLlmReason({
    env: process.env,
    runtimeRoots: defaultRuntimeRoots({ argv }),
  });
  if (reason) {
    console.error(`aifs: skipping cargo engine-llm (${reason}). pnpm llama / llama:cuda rebuilds.`);
    process.exit(0);
  }
  const child = spawn(process.execPath, [wrap, 'cargo', 'engine-llm', ...argv], {
    stdio: 'inherit',
    env: process.env,
  });
  child.on('error', (error) => {
    console.error(error.message);
    process.exit(1);
  });
  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 1);
  });
}
