#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { parseLlmFeatures } from './llm-features.mjs';

const [features, command, ...args] = process.argv.slice(2);
if (!features || !command) {
  console.error(
    'usage: with-llm-features.mjs <cuda|vulkan|metal[,...]> <command> [args...]',
  );
  process.exit(2);
}

const parsed = parseLlmFeatures(features);
if (parsed.length === 0) {
  console.error(
    `aifs: unknown LLM features "${features}" (use cuda, vulkan, and/or metal)`,
  );
  process.exit(2);
}

const env = { ...process.env, AIFS_LLM_FEATURES: parsed.join(',') };
const child = spawn(command, args, {
  stdio: 'inherit',
  env,
  shell: process.platform === 'win32',
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
