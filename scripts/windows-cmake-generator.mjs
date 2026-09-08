import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';

export const VS_2026 = 'Visual Studio 18 2026';
export const VS_2022 = 'Visual Studio 17 2022';
export const VS_2019 = 'Visual Studio 16 2019';
export const VS_2017 = 'Visual Studio 15 2017';

const VS_MAJOR_TO_GENERATOR = new Map([
  [18, VS_2026],
  [17, VS_2022],
  [16, VS_2019],
  [15, VS_2017],
]);

const VS_FALLBACK_MAJORS = [17, 16, 15];

/** Map a Visual Studio major version to the cmake-rs generator name. */
export function cmakeGeneratorForVsVersion(major) {
  if (major == null || Number.isNaN(major)) return undefined;
  return VS_MAJOR_TO_GENERATOR.get(Number(major));
}

/** Parse `cmake --help` generator names (`* Name  = description`). */
export function parseCmakeHelpGenerators(helpText) {
  const names = [];
  for (const line of helpText.split('\n')) {
    const match = line.match(/^\s*\*?\s*(.+?)\s{2,}=\s/);
    if (match) names.push(match[1].trim());
  }
  return names;
}

/** Parse `cmake -E capabilities` generator names. */
export function parseCmakeCapabilitiesGenerators(jsonText) {
  const caps = JSON.parse(jsonText);
  if (!Array.isArray(caps.generators)) return [];
  return caps.generators
    .map((generator) => generator?.name)
    .filter((name) => typeof name === 'string' && name.length > 0);
}

/**
 * Choose CMAKE_GENERATOR when cmake-rs would pass a Visual Studio name CMake
 * does not list (VS 2026 + CMake < 4.2).
 */
export function pickWindowsCmakeGenerator({
  platform,
  existingGenerator,
  listedGenerators,
  latestVsMajor,
  installedVsMajors,
  hasNinja,
}) {
  if (platform !== 'win32') return undefined;
  if (existingGenerator) return undefined;

  const listed = new Set(listedGenerators);
  const wanted = cmakeGeneratorForVsVersion(latestVsMajor);
  if (!wanted || listed.has(wanted)) return undefined;

  const installed = new Set(installedVsMajors);
  for (const major of VS_FALLBACK_MAJORS) {
    const name = cmakeGeneratorForVsVersion(major);
    if (name && listed.has(name) && installed.has(major)) return name;
  }
  if (listed.has('Ninja') && hasNinja) return 'Ninja';
  for (const major of VS_FALLBACK_MAJORS) {
    const name = cmakeGeneratorForVsVersion(major);
    if (name && listed.has(name)) return name;
  }
  return undefined;
}

function usesShell(command) {
  return process.platform === 'win32' && !/[\\/]/.test(command);
}

function commandOutput(command, args) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    shell: usesShell(command),
    windowsHide: true,
  });
  if (result.status !== 0) {
    throw new Error(
      `${command} ${args.join(' ')} exited ${result.status}: ${result.stderr || result.stdout || ''}`,
    );
  }
  return (result.stdout || '').trim();
}

function commandExists(command) {
  const result = spawnSync(command, ['--version'], {
    encoding: 'utf8',
    shell: usesShell(command),
    windowsHide: true,
  });
  return result.status === 0;
}

function vswherePath() {
  const roots = [process.env['ProgramFiles(x86)'], process.env.ProgramFiles].filter(Boolean);
  for (const root of roots) {
    const candidate = join(root, 'Microsoft Visual Studio', 'Installer', 'vswhere.exe');
    if (existsSync(candidate)) return candidate;
  }
  return undefined;
}

function parseVsMajor(installationVersion) {
  const major = Number.parseInt(String(installationVersion).split('.')[0], 10);
  return Number.isNaN(major) ? undefined : major;
}

function queryVswhere(vswhere, extraArgs) {
  try {
    const output = commandOutput(vswhere, [
      '-nologo',
      '-prerelease',
      '-products',
      '*',
      '-requires',
      'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
      ...extraArgs,
      '-property',
      'installationVersion',
    ]);
    return output
      .split(/\r?\n/)
      .map((line) => parseVsMajor(line.trim()))
      .filter((major) => major != null);
  } catch {
    return [];
  }
}

function listCmakeGenerators() {
  try {
    return parseCmakeCapabilitiesGenerators(commandOutput('cmake', ['-E', 'capabilities']));
  } catch {
    try {
      return parseCmakeHelpGenerators(commandOutput('cmake', ['--help']));
    } catch {
      return [];
    }
  }
}

function inspectWindowsBuildTools() {
  const vswhere = vswherePath();
  const latest = vswhere ? queryVswhere(vswhere, ['-latest']) : [];
  const installed = vswhere
    ? queryVswhere(vswhere, ['-version', '[15.0,19.0)'])
    : latest;
  return {
    latestVsMajor: latest[0],
    installedVsMajors: [...new Set(installed)],
    hasNinja: commandExists('ninja'),
  };
}

/** Set CMAKE_GENERATOR when Windows CMake cannot create the VS version cmake-rs picks. */
export function applyWindowsCmakeGenerator({
  env = process.env,
  platform = process.platform,
  listGenerators = listCmakeGenerators,
  inspectBuildTools = inspectWindowsBuildTools,
  log = console.error,
} = {}) {
  if (platform !== 'win32' || env.CMAKE_GENERATOR) return undefined;
  const tools = inspectBuildTools();
  const generator = pickWindowsCmakeGenerator({
    platform,
    existingGenerator: env.CMAKE_GENERATOR,
    listedGenerators: listGenerators(),
    latestVsMajor: tools.latestVsMajor,
    installedVsMajors: tools.installedVsMajors ?? [],
    hasNinja: Boolean(tools.hasNinja),
  });
  if (!generator) return undefined;
  env.CMAKE_GENERATOR = generator;
  const wanted = cmakeGeneratorForVsVersion(tools.latestVsMajor);
  log(
    `aifs: CMAKE_GENERATOR=${generator} (CMake does not list ${wanted ?? 'the Visual Studio generator cmake-rs would pick'})`,
  );
  return generator;
}
