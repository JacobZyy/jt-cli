import fs from 'node:fs';
import path from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const markerName = '.jt-codex-dependencies.json';
const excluded = new Set(['.git', '.codex', '.codegraph', 'node_modules', '.jt-codex-dependencies.lock', 'target', 'dist', 'build', 'coverage']);
const caches = new Set(['.cache', '.vite', '.vite-temp']);
const lifecycle = ['preinstall', 'install', 'postinstall', 'prepare'];
const hash = value => createHash('sha256').update(value).digest('hex');
const inside = (root, file) => file === root || file.startsWith(`${root}${path.sep}`);
const exists = file => { try { fs.lstatSync(file); return true; } catch (error) { if (error.code === 'ENOENT') return false; throw error; } };

function projectFiles(root) {
  try {
    return execFileSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })
      .split('\0').filter(file => file && !file.split('/').some(part => excluded.has(part)) && fs.existsSync(path.join(root, file))).sort();
  } catch {
    const files = [];
    const walk = relative => {
      for (const entry of fs.readdirSync(path.join(root, relative), { withFileTypes: true })) {
        if (excluded.has(entry.name)) continue;
        const file = path.join(relative, entry.name);
        if (entry.isDirectory()) walk(file);
        else files.push(file);
      }
    };
    walk('');
    return files.sort();
  }
}

function normalize(value, root) {
  if (typeof value === 'string') return value.split(root).join('<project>');
  if (Array.isArray(value)) return value.map(item => normalize(item, root));
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(key => [key, normalize(value[key], root)]));
  return value;
}

function inputs(root, manager, version, installArgs) {
  const files = [...new Set([...projectFiles(root), ...['package.json', 'package-lock.json', 'npm-shrinkwrap.json', 'pnpm-lock.yaml', 'pnpm-workspace.yaml', '.npmrc', '.pnpmfile.cjs', 'pnpmfile.cjs'].filter(file => exists(path.join(root, file)))])].sort();
  const manifests = files.filter(file => path.basename(file) === 'package.json').map(file => {
    if (!inside(root, fs.realpathSync(path.join(root, file)))) throw new Error('package manifest points outside project');
    return [file, JSON.parse(fs.readFileSync(path.join(root, file), 'utf8'))];
  });
  const packageJson = manifests.find(([file]) => file === 'package.json')?.[1];
  if (!packageJson) throw new Error('package.json is missing');
  const declared = packageJson.packageManager?.match(/^(npm|pnpm)@(\d+\.\d+\.\d+(?:-[^+]+)?)(?:\+.*)?$/);
  if (declared && (declared[1] !== manager || declared[2] !== version)) throw new Error(`packageManager requires ${declared[1]}@${declared[2]}; current ${manager}@${version}`);
  let local = Boolean(packageJson.workspaces || fs.existsSync(path.join(root, 'pnpm-workspace.yaml')));
  let external = false;
  const localPaths = new Set(local ? manifests.filter(([file]) => file !== 'package.json').map(([file]) => path.join(root, path.dirname(file))) : []);
  for (const [file, manifest] of manifests) {
    for (const dependencies of ['dependencies', 'devDependencies', 'optionalDependencies', 'peerDependencies']) {
      for (const specifier of Object.values(manifest[dependencies] || {})) {
        if (typeof specifier !== 'string') continue;
        if (/^(workspace:|file:|link:)/.test(specifier)) {
          local = true;
          if (!specifier.startsWith('workspace:')) {
            const target = path.resolve(root, path.dirname(file), specifier.replace(/^(file:|link:)/, ''));
            if (!inside(root, target) || !fs.existsSync(target) || !inside(root, fs.realpathSync(target))) external = true;
            else localPaths.add(target);
          }
        }
      }
    }
  }
  const scripts = manifests.some(([, manifest]) => lifecycle.some(name => manifest.scripts?.[name]));
  const selected = new Set(local || scripts ? files : files.filter(file =>
    path.basename(file) === 'package.json' || ['package-lock.json', 'npm-shrinkwrap.json', 'pnpm-lock.yaml', 'pnpm-workspace.yaml', '.npmrc', '.pnpmfile.cjs', 'pnpmfile.cjs'].includes(path.basename(file))));
  const visited = new Set();
  const localExclusions = new Set(['.git', '.codex', '.codegraph', 'node_modules', '.jt-codex-dependencies.lock', ...caches]);
  const collect = absolute => {
    if (!inside(root, fs.realpathSync(absolute))) { external = true; return; }
    const stat = fs.lstatSync(absolute);
    if (stat.isSymbolicLink()) {
      selected.add(path.relative(root, absolute));
      const target = fs.realpathSync(absolute);
      if (!visited.has(target)) collect(target);
    } else if (stat.isDirectory()) {
      if (visited.has(absolute)) return;
      visited.add(absolute);
      for (const entry of fs.readdirSync(absolute)) if (!localExclusions.has(entry)) collect(path.join(absolute, entry));
    } else if (stat.isFile()) {
      const relative = path.relative(root, absolute);
      selected.add(relative);
      if (path.basename(absolute) === 'package.json' && !manifests.some(([file]) => file === relative)) external = true;
    }
    else throw new Error('local dependency contains unsupported special file');
  };
  for (const absolute of localPaths) collect(absolute);
  const content = [...selected].sort().map(file => {
    const absolute = path.join(root, file);
    const stat = fs.lstatSync(absolute);
    return [file, stat.isSymbolicLink() ? fs.readlinkSync(absolute) : hash(fs.readFileSync(absolute))];
  });
  const configuration = JSON.parse(execFileSync(manager, ['config', 'list', '--json'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }));
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    /^(npm_config_|pnpm_|NPM_CONFIG_|PNPM_|NODE_|CC$|CXX$|CFLAGS$|CXXFLAGS$|CPPFLAGS$|LDFLAGS$)/.test(key)).sort(([a], [b]) => a.localeCompare(b)));
  const runtime = { node: process.version, abi: process.versions.modules, platform: process.platform, arch: process.arch, executable: fs.realpathSync(process.execPath), manager, version };
  return {
    fingerprint: hash(JSON.stringify({ version: 1, content, configuration: normalize(configuration, root), environment, runtime, installArgs })),
    manifests,
    scripts,
    external,
    native: [...selected].filter(file => file.endsWith('.node')).map(file => path.join(root, file)),
    modules: [...new Set(manifests.map(([file]) => path.join(path.dirname(file), 'node_modules')))].sort(),
  };
}

function inspect(root, modules) {
  const entries = [];
  const generations = [];
  const native = [];
  for (const relative of modules) {
    const directory = path.join(root, relative);
    if (!exists(directory)) continue;
    if (fs.lstatSync(directory).isSymbolicLink()) throw new Error('refusing shared node_modules symlink');
    const walk = directory => {
      for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
        if (entry.name === markerName || caches.has(entry.name)) continue;
        const file = path.join(directory, entry.name);
        const relative = path.relative(root, file);
        const stat = fs.lstatSync(file);
        generations.push([relative, stat.size, stat.mtimeMs, stat.ctimeMs]);
        if (entry.isSymbolicLink()) {
          const link = fs.readlinkSync(file);
          if (path.isAbsolute(link) || !inside(root, path.resolve(directory, link)) || !inside(root, fs.realpathSync(file))) throw new Error('dependency symlink escapes project or uses an absolute path');
          entries.push([relative, 'symlink', link]);
        } else if (entry.isDirectory()) {
          entries.push([relative, 'directory']);
          walk(file);
        } else if (entry.isFile()) {
          const nativeFile = entry.name.endsWith('.node');
          if (nativeFile) native.push(file);
          const metadata = entry.name === 'package.json' || entry.name === '.modules.yaml' || entry.name === '.package-lock.json' || (entry.name === 'lock.yaml' && path.basename(directory) === '.pnpm');
          const launcher = path.basename(directory) === '.bin';
          if (entry.name === 'package.json') {
            const manifest = JSON.parse(fs.readFileSync(file, 'utf8'));
            for (const [key, current] of [['os', process.platform], ['cpu', process.arch]]) {
              const values = manifest[key];
              if (Array.isArray(values) && (values.includes(`!${current}`) || (values.some(value => !value.startsWith('!')) && !values.includes(current) && !values.includes('any')))) throw new Error('installed dependency platform/architecture is incompatible');
            }
          }
          entries.push([relative, 'file', launcher ? Buffer.byteLength(normalize(fs.readFileSync(file, 'utf8'), root)) : stat.size, nativeFile ? hash(fs.readFileSync(file)) : metadata || launcher ? hash(normalize(fs.readFileSync(file, 'utf8'), root)) : null]);
        } else throw new Error('dependency contains unsupported special file');
      }
    };
    walk(directory);
  }
  if (!exists(path.join(root, 'node_modules'))) throw new Error('node_modules is missing');
  return { digest: hash(JSON.stringify(entries)), generation: hash(JSON.stringify(generations)), native };
}

function stamped(root, input, layout) {
  try {
    const stamp = JSON.parse(fs.readFileSync(path.join(root, 'node_modules', markerName), 'utf8'));
    return stamp.version === 1 && stamp.fingerprint === input.fingerprint && stamp.layout === layout.digest;
  } catch { return false; }
}

function writeStamp(root, input, layout) {
  const native = [...new Set([...layout.native, ...input.native])];
  if (native.length) {
    const probe = spawnSync(process.execPath, ['-e', 'for (const file of JSON.parse(require("node:fs").readFileSync(0, "utf8"))) require(file);'], { cwd: root, input: JSON.stringify(native), stdio: ['pipe', 'ignore', 'ignore'], timeout: 10_000 });
    if (probe.error || probe.status !== 0) throw new Error('native dependency ABI probe failed');
  }
  const marker = path.join(root, 'node_modules', markerName);
  const temporary = `${marker}.${randomUUID()}`;
  try {
    fs.writeFileSync(temporary, `${JSON.stringify({ version: 1, fingerprint: input.fingerprint, layout: layout.digest })}\n`, { flag: 'wx' });
    fs.renameSync(temporary, marker);
  } finally { fs.rmSync(temporary, { force: true }); }
}

function rebaseLaunchers(directory, mainRoot, root) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) rebaseLaunchers(file, mainRoot, root);
    else if (entry.isFile() && path.basename(directory) === '.bin') {
      const content = fs.readFileSync(file);
      if (content.includes(Buffer.from(mainRoot))) {
        if (content.includes(0)) throw new Error('dependency launcher contains embedded project path in binary');
        fs.writeFileSync(file, content.toString().split(mainRoot).join(root));
      }
    }
  }
}

function cloneDependencies(source, target) {
  try {
    fs.cpSync(source, target, { recursive: true, mode: fs.constants.COPYFILE_FICLONE_FORCE, verbatimSymlinks: true, preserveTimestamps: true, filter: source => !caches.has(path.basename(source)) && path.basename(source) !== markerName });
  } catch (error) {
    if (!['darwin', 'linux'].includes(process.platform) || !['ENOSYS', 'ENOTSUP', 'EOPNOTSUPP'].includes(error.code)) throw error;
    fs.rmSync(target, { recursive: true, force: true });
    execFileSync('/bin/cp', process.platform === 'darwin' ? ['-cR', '-pP', source, target] : ['--reflink=always', '--archive', source, target], { stdio: ['ignore', 'pipe', 'pipe'] });
    const clean = directory => {
      for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
        const file = path.join(directory, entry.name);
        if (caches.has(entry.name) || entry.name === markerName) fs.rmSync(file, { recursive: true, force: true });
        else if (entry.isDirectory()) clean(file);
      }
    };
    clean(target);
  }
}

function setupUnlocked(root, mainRoot, manager, installArgs) {
  if (!['npm', 'pnpm'].includes(manager)) throw new Error('supported package managers: npm, pnpm');
  for (const directory of ['node_modules', ...projectFiles(root).filter(file => path.basename(file) === 'package.json').map(file => path.join(path.dirname(file), 'node_modules'))]) {
    const absolute = path.join(root, directory);
    if (exists(absolute) && fs.lstatSync(absolute).isSymbolicLink()) throw new Error('refusing shared node_modules symlink');
  }
  const version = execFileSync(manager, ['--version'], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  const input = inputs(root, manager, version, installArgs);
  try {
    const layout = inspect(root, input.modules);
    if (!input.scripts && !input.external && stamped(root, input, layout)) {
      console.log('Codex dependencies: preserved compatible installed environment.');
      return 'preserved';
    }
  } catch {}
  if (root !== mainRoot && !input.scripts && !input.external) {
    const temporary = [];
    try {
      const source = inputs(mainRoot, manager, version, installArgs);
      if (source.fingerprint !== input.fingerprint || source.scripts || source.external) throw new Error('dependency inputs or install lifecycle differ');
      const layout = inspect(mainRoot, source.modules);
      if (!stamped(mainRoot, source, layout)) throw new Error('main dependencies lack compatible install/runtime evidence');
      const modules = source.modules.filter(relative => exists(path.join(mainRoot, relative)));
      if (modules.some(relative => exists(path.join(root, relative)))) throw new Error('existing dependency directory needs normal validation/install');
      for (const relative of modules) {
        const target = path.join(root, relative);
        fs.mkdirSync(path.dirname(target), { recursive: true });
        const stage = `${target}.jt-codex-${randomUUID()}`;
        temporary.push(stage);
        cloneDependencies(path.join(mainRoot, relative), stage);
        rebaseLaunchers(stage, mainRoot, root);
        if (fs.statSync(path.join(mainRoot, relative)).ino === fs.statSync(stage).ino) throw new Error('dependency directory is not independent');
      }
      if (inputs(mainRoot, manager, version, installArgs).fingerprint !== source.fingerprint || inspect(mainRoot, source.modules).generation !== layout.generation) throw new Error('main dependencies changed during copy');
      for (let index = 0; index < modules.length; index++) {
        const target = path.join(root, modules[index]);
        if (exists(target)) throw new Error('dependency directory appeared during copy');
        fs.renameSync(temporary[index], target);
      }
      const cloned = inspect(root, input.modules);
      writeStamp(root, input, cloned);
      console.log('Codex dependencies: reused independent copy-on-write dependencies.');
      return 'reused';
    } catch (error) {
      console.log(`Codex dependencies: normal install required (${error.message}).`);
    } finally {
      for (const directory of temporary) fs.rmSync(directory, { recursive: true, force: true });
    }
  }
  const installed = spawnSync(manager, installArgs, { cwd: root, stdio: 'inherit' });
  if (installed.error) throw installed.error;
  if (installed.status !== 0) throw new Error(`${manager} install failed with exit status ${installed.status ?? installed.signal}`);
  fs.mkdirSync(path.join(root, 'node_modules'), { recursive: true });
  try {
    const current = inputs(root, manager, version, installArgs);
    if (current.external) throw new Error('local dependency layout is external or not fully discoverable');
    if (current.scripts) throw new Error('install lifecycle outputs cannot be proven reusable');
    writeStamp(root, current, inspect(root, current.modules));
  } catch (error) {
    console.log(`Codex dependencies: installed; reuse unavailable (${error.message}).`);
  }
  return 'installed';
}

export function setupDependencies(root, mainRoot, manager, installArgs) {
  root = fs.realpathSync(root);
  mainRoot = fs.realpathSync(mainRoot);
  const lock = path.join(root, '.jt-codex-dependencies.lock');
  try { fs.mkdirSync(lock, { mode: 0o700 }); }
  catch (error) {
    if (error.code !== 'EEXIST') throw error;
    let owner = 'unknown owner';
    try {
      if (!fs.lstatSync(lock).isSymbolicLink()) {
        const data = JSON.parse(fs.readFileSync(path.join(lock, 'owner.json'), 'utf8'));
        if (Number.isSafeInteger(data.pid) && data.pid > 0) owner = `PID ${data.pid}`;
      }
    } catch {}
    throw new Error(`dependency setup lock exists: ${lock} (${owner}); verify owner before removing lock`);
  }
  const owned = fs.lstatSync(lock);
  try {
    fs.writeFileSync(path.join(lock, 'owner.json'), `${JSON.stringify({ pid: process.pid, root, startedAt: new Date().toISOString() })}\n`, { flag: 'wx' });
    return setupUnlocked(root, mainRoot, manager, installArgs);
  } finally {
    if (exists(lock)) {
      const current = fs.lstatSync(lock);
      if (current.dev === owned.dev && current.ino === owned.ino && !current.isSymbolicLink()) fs.rmSync(lock, { recursive: true, force: true });
    }
  }
}

if (process.argv[1] === '-' || (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]))) {
  setupDependencies(process.argv[2], process.argv[3], process.argv[4], process.argv.slice(5));
}
