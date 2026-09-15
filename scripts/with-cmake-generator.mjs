#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { applyLlamaCudaBuildEnv } from './llama-cuda-env.mjs';
import { appendEngineLlmFeatures, applyLinuxLlamaCxx } from './llm-features.mjs';
import { applyPackagedGgmlEnv } from './packaged-ggml.mjs';
import {
  defaultStagePlan,
  formatStagedPayloadLog,
  shouldStageAfterEngineLlm,
  stageLlmPayloadFromDir,
} from './stage-llm-payload.mjs';
import { applyWindowsCmakeGenerator } from './windows-cmake-generator.mjs';

applyWindowsCmakeGenerator();

const argv = process.argv.slice(2);
const packaged = argv[0] === '--packaged';
if (packaged) {
  argv.shift();
  applyPackagedGgmlEnv({ packaged: true });
}

let forwarded;
try {
  forwarded = appendEngineLlmFeatures(argv);
} catch (error) {
  console.error(error.message);
  process.exit(2);
}
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
  if (code === 0 && shouldStageAfterEngineLlm(command, forwarded)) {
    try {
      const { accel, copiedLibNames } = stageLlmPayloadFromDir(
        defaultStagePlan({ argv: forwarded }),
      );
      console.error(formatStagedPayloadLog(accel, copiedLibNames));
    } catch (error) {
      console.error(error.message);
      process.exit(1);
      return;
    }
  }
  process.exit(code ?? 1);
});
