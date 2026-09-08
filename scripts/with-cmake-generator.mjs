#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { applyWindowsCmakeGenerator } from './windows-cmake-generator.mjs';

applyWindowsCmakeGenerator();

const [command, ...args] = process.argv.slice(2);
if (!command) {
  console.error('usage: with-cmake-generator.mjs <command> [args...]');
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
