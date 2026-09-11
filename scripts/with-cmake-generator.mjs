#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { applyLlamaCudaBuildEnv } from './llama-cuda-env.mjs';
import { appendEngineLlmFeatures, applyLinuxLlamaCxx } from './llm-features.mjs';
import { applyPackagedGgmlEnv } from './packaged-ggml.mjs';
import { applyWindowsCmakeGenerator } from './windows-cmake-generator.mjs';

applyWindowsCmakeGenerator();

const argv = process.argv.slice(2);
const packaged = argv[0] === '--packaged';
if (packaged) {
  argv.shift();
  applyPackagedGgmlEnv({ packaged: true });
}

const forwarded = appendEngineLlmFeatures(argv);
applyLinuxLlamaCxx({ argv: forwarded });
applyLlamaCudaBuildEnv({ argv: forwarded });

const [command, ...args] = forwarded;
if (!command) {
  console.error('usage: with-cmake-generator.mjs [--packaged] <command> [args...]');
  process.exit(2);
}

const child = spawn(command, args, {
  stdio: 'inherit',
  env: process.env,
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
