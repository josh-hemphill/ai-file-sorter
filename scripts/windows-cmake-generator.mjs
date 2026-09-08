import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';

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
const NINJA_REL = join('Common7', 'IDE', 'CommonExtensions', 'Microsoft', 'CMake', 'Ninja', 'ninja.exe');

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

/** Parse `cmd /c set` output into an environment map. */
export function parseCmdSetOutput(text) {
  const env = {};
  for (const line of text.split(/\r?\n/)) {
    const eq = line.indexOf('=');
    if (eq <= 0) continue;
    const key = line.slice(0, eq);
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;
    env[key] = line.slice(eq + 1);
  }
  return env;
}

/**
 * Choose how to run cmake-rs on Windows when CMake cannot create the Visual
 * Studio generator for the installed VS version (VS 2026 + CMake < 4.2).
 *
 * Never force Visual Studio 17 2022 unless vswhere shows VS 2022: that
 * replaces "unknown generator" with "could not find any instance of Visual
 * Studio" on VS 2026-only machines.
 */
export function planWindowsLlamaBuild({
  platform,
  existingGenerator,
  listedGenerators,
  latestVsMajor,
  installedVsMajors,
  hasNinja,
}) {
  if (platform !== 'win32') return { kind: 'noop' };
  if (existingGenerator) return { kind: 'keep', generator: existingGenerator };

  const listed = new Set(listedGenerators);
  const installed = new Set(installedVsMajors);
  const wanted = cmakeGeneratorForVsVersion(latestVsMajor);
  if (wanted && listed.has(wanted)) return { kind: 'noop' };

  const needsOverride = !wanted || !listed.has(wanted);
  if (!needsOverride) return { kind: 'noop' };

  for (const major of VS_FALLBACK_MAJORS) {
    const name = cmakeGeneratorForVsVersion(major);
    if (name && listed.has(name) && installed.has(major)) {
      return {
        kind: 'set',
        generator: name,
        reason: `CMake does not list ${wanted ?? VS_2026}; VS ${major} is installed`,
      };
    }
  }
  if (listed.has('Ninja') && hasNinja) {
    return {
      kind: 'set',
      generator: 'Ninja',
      reason: `CMake does not list ${wanted ?? VS_2026}; using Ninja with the installed MSVC toolchain`,
    };
  }
  return {
    kind: 'error',
    message: [
      `aifs: CMake does not list ${wanted ?? VS_2026} (needs CMake 4.2+) and no usable fallback was found.`,
      'Install Ninja (`winget install Ninja-build.Ninja`) or upgrade CMake (`winget upgrade Kitware.CMake`), then re-run pnpm llama.',
      'If a previous attempt cached the wrong generator, delete target\\debug\\build\\llama-cpp-sys-2-* and retry.',
    ].join('\n'),
  };
}

/** CMAKE_GENERATOR value to set, if any. */
export function pickWindowsCmakeGenerator(options) {
  const plan = planWindowsLlamaBuild(options);
  return plan.kind === 'set' ? plan.generator : undefined;
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

function vswhereArgs(extraArgs) {
  return ['-nologo', '-prerelease', '-products', '*', ...extraArgs];
}

function queryVswhereProperty(vswhere, extraArgs, property) {
  try {
    return commandOutput(vswhere, [...vswhereArgs(extraArgs), '-property', property]);
  } catch {
    return '';
  }
}

function firstNonEmptyLine(text) {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean);
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

function findBundledNinja(vswhere, installPath) {
  if (vswhere) {
    try {
      const found = firstNonEmptyLine(
        commandOutput(vswhere, [
          ...vswhereArgs(['-latest']),
          '-find',
          'Common7\\IDE\\CommonExtensions\\Microsoft\\CMake\\Ninja\\ninja.exe',
        ]),
      );
      if (found && existsSync(found)) return found;
    } catch {
      // Fall through to the install-path join.
    }
  }
  if (!installPath) return undefined;
  const bundled = join(installPath, NINJA_REL);
  return existsSync(bundled) ? bundled : undefined;
}

function inspectWindowsBuildTools() {
  const vswhere = vswherePath();
  const vcTools = ['-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64'];
  let latestVersion = vswhere
    ? queryVswhereProperty(vswhere, [...vcTools, '-latest'], 'installationVersion')
    : '';
  let installPath = vswhere
    ? queryVswhereProperty(vswhere, [...vcTools, '-latest'], 'installationPath')
    : '';
  if (vswhere && !latestVersion) {
    latestVersion = queryVswhereProperty(vswhere, ['-latest'], 'installationVersion');
    installPath = queryVswhereProperty(vswhere, ['-latest'], 'installationPath');
  }
  const latestVsMajor = parseVsMajor(latestVersion);
  const installedText = vswhere
    ? queryVswhereProperty(vswhere, ['-version', '[15.0,19.0)'], 'installationVersion')
    : '';
  const installedVsMajors = installedText
    .split(/\r?\n/)
    .map((line) => parseVsMajor(line.trim()))
    .filter((major) => major != null);
  if (latestVsMajor != null) installedVsMajors.push(latestVsMajor);

  const bundledNinja = findBundledNinja(vswhere, installPath);
  const ninjaOnPath = commandExists('ninja');

  return {
    latestVsMajor,
    installedVsMajors: [...new Set(installedVsMajors)],
    hasNinja: ninjaOnPath || Boolean(bundledNinja),
    ninjaPath: bundledNinja,
    vsInstallPath: installPath || undefined,
  };
}

function vcvarsArch(env) {
  const arch = String(env.PROCESSOR_ARCHITECTURE || '').toUpperCase();
  if (arch === 'ARM64') return 'arm64';
  return 'amd64';
}

function windowsDirname(filePath) {
  const normalized = filePath.replace(/[/\\]+$/, '');
  const idx = Math.max(normalized.lastIndexOf('\\'), normalized.lastIndexOf('/'));
  return idx <= 0 ? normalized : normalized.slice(0, idx);
}

function prependPath(env, directory, platform) {
  const key = Object.keys(env).find((name) => name.toLowerCase() === 'path') || 'PATH';
  const sep = platform === 'win32' ? ';' : ':';
  env[key] = env[key] ? `${directory}${sep}${env[key]}` : directory;
}

/** Merge `vcvarsall.bat` into env so the Ninja generator can find cl.exe. */
export function loadMsvcEnvironment(env, vsInstallPath, log = console.error) {
  if (!vsInstallPath) return false;
  const vcvars = join(vsInstallPath, 'VC', 'Auxiliary', 'Build', 'vcvarsall.bat');
  if (!existsSync(vcvars)) {
    log(`aifs: missing ${vcvars}`);
    return false;
  }
  const arch = vcvarsArch(env);
  const comspec = env.ComSpec || 'cmd.exe';
  const result = spawnSync(comspec, ['/d', '/s', '/c', `call "${vcvars}" ${arch} >nul && set`], {
    encoding: 'utf8',
    windowsHide: true,
    env,
  });
  if (result.status !== 0) {
    log(`aifs: vcvarsall.bat ${arch} failed: ${(result.stderr || result.stdout || '').trim()}`);
    return false;
  }
  const vars = parseCmdSetOutput(result.stdout);
  for (const [key, value] of Object.entries(vars)) env[key] = value;
  return true;
}

const CMAKE_CACHE_RELATIVE = ['out/build', 'build', 'out'];

/** Drop llama-cpp-sys-2 CMake caches so a new CMAKE_GENERATOR can take effect. */
export function clearLlamaCppSysCmakeCache(repoRoot, log = console.error) {
  let cleared = 0;
  for (const profile of ['debug', 'release']) {
    const buildRoot = join(repoRoot, 'target', profile, 'build');
    if (!existsSync(buildRoot)) continue;
    for (const name of readdirSync(buildRoot)) {
      if (!name.startsWith('llama-cpp-sys-2-')) continue;
      for (const rel of CMAKE_CACHE_RELATIVE) {
        const cache = join(buildRoot, name, rel, 'CMakeCache.txt');
        if (!existsSync(cache)) continue;
        rmSync(dirname(cache), { recursive: true, force: true });
        cleared += 1;
      }
    }
  }
  if (cleared) log(`aifs: cleared ${cleared} stale llama-cpp-sys-2 CMake cache(s)`);
  return cleared;
}

/** Set CMAKE_GENERATOR and MSVC env when Windows CMake cannot create cmake-rs's VS generator. */
export function applyWindowsCmakeGenerator({
  env = process.env,
  platform = process.platform,
  listGenerators = listCmakeGenerators,
  inspectBuildTools = inspectWindowsBuildTools,
  loadMsvc = loadMsvcEnvironment,
  clearCache = clearLlamaCppSysCmakeCache,
  repoRoot = process.cwd(),
  log = console.error,
} = {}) {
  if (platform !== 'win32') return { kind: 'noop' };
  const tools = inspectBuildTools();
  const plan = env.CMAKE_GENERATOR
    ? { kind: 'keep', generator: env.CMAKE_GENERATOR }
    : planWindowsLlamaBuild({
        platform,
        listedGenerators: listGenerators(),
        latestVsMajor: tools.latestVsMajor,
        installedVsMajors: tools.installedVsMajors ?? [],
        hasNinja: Boolean(tools.hasNinja),
      });
  if (plan.kind === 'error') {
    log(plan.message);
    return plan;
  }
  if (plan.kind === 'set') {
    env.CMAKE_GENERATOR = plan.generator;
    log(`aifs: CMAKE_GENERATOR=${plan.generator} (${plan.reason})`);
    clearCache(repoRoot, log);
  }
  const generator = plan.kind === 'keep' ? plan.generator : env.CMAKE_GENERATOR;
  if (generator !== 'Ninja') return plan;
  if (tools.ninjaPath) {
    prependPath(env, windowsDirname(tools.ninjaPath), platform);
    log(`aifs: using ${tools.ninjaPath}`);
  }
  if (!loadMsvc(env, tools.vsInstallPath, log)) {
    log(
      'aifs: could not load vcvarsall.bat; Ninja may fail to find cl.exe. Use an x64 Native Tools Command Prompt, or upgrade CMake to 4.2+.',
    );
  }
  return plan;
}
