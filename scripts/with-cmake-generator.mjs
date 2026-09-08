#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { applyWindowsCmakeGenerator } from './windows-cmake-generator.mjs';

const plan = applyWindowsCmakeGenerator();
if (plan?.kind === 'error') process.exit(1);

const [command, ...args] = process.argv.slice(2);
if (!command) {
  console.error('usage: with-cmake-generator.mjs <command> [args...]');
  process.exit(2);
}

const child = spawn(command, args, {
  stdio: 'inherit',
  env: process.env,
  windowsHide: true,
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
