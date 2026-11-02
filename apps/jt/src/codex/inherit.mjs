import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';

const manifestName = '.worktreeinclude';
const excludedNames = new Set(['.git', '.codegraph', 'node_modules', 'target', 'dist', 'build', 'cache', 'logs', 'reports']);
const excludedSuffixes = ['.lock', '.log', '.db', '.db-wal', '.db-shm', '.sqlite', '.sqlite3', '.sqlite-wal', '.sqlite-shm'];
const lefthookStems = ['lefthook-local', '.lefthook-local', '.config/lefthook-local'];
const environment = { ...process.env, LC_ALL: 'C' };
for (const variable of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR']) delete environment[variable];

function git(root, args) {
  const result = spawnSync('git', args, { cwd: root, env: environment, maxBuffer: 64 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(result.stderr.toString('utf8').trim());
  return new TextDecoder('utf-8', { fatal: true }).decode(result.stdout);
}

function inspect(file) {
  try {
    return fs.lstatSync(file);
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

function readConfiguration(file) {
  const metadata = inspect(file);
  if (!metadata) return null;
  if (!metadata.isFile()) throw new Error(`refusing non-file ${file}`);
  return new TextDecoder('utf-8', { fatal: true }).decode(fs.readFileSync(file));
}

function prepareParents(root, relative, create) {
  let directory = root;
  for (const component of relative.split('/').slice(0, -1)) {
    directory = path.join(directory, component);
    const metadata = inspect(directory);
    if (metadata) {
      if (!metadata.isDirectory()) throw new Error(`refusing non-directory ${directory}`);
    } else {
      if (!create) return false;
      try {
        fs.mkdirSync(directory);
      } catch (error) {
        if (error.code !== 'EEXIST') throw error;
        if (!inspect(directory)?.isDirectory()) throw new Error(`refusing non-directory ${directory}`);
      }
    }
  }
  return true;
}

function configurationIsShadowed(root, source, relative) {
  const stem = relative.slice(0, relative.length - path.posix.extname(relative).length);
  if (lefthookStems.includes(stem)) {
    let formats = 0;
    for (const candidate of lefthookStems) {
      for (const extension of ['yml', 'yaml', 'toml', 'json', 'jsonc']) {
        const alias = `${candidate}.${extension}`;
        if (inspect(path.join(root, alias))) return true;
        if (inspect(path.join(source, alias))) formats++;
      }
    }
    if (formats > 1) throw new Error('multiple Lefthook local override formats or paths; select one in the primary worktree');
  }
  if (relative === '.nlab/cli.local.json') {
    return Boolean(inspect(path.join(source, '.nlab/nlab-api.local.json')) || inspect(path.join(root, '.nlab/nlab-api.local.json')));
  }
  if (relative !== '.nlab/nlab-api.local.json') return false;
  const content = readConfiguration(path.join(root, '.nlab/cli.local.json'));
  if (content === null) return false;
  let legacy;
  try {
    legacy = JSON.parse(content);
  } catch {
    return false;
  }
  return legacy !== null && typeof legacy === 'object' && !Array.isArray(legacy)
    && Object.keys(legacy).length === 2 && legacy.version === 1
    && ['nlab', 'jt', 'nlab-api'].includes(legacy.runner);
}

function copyFile(source, destination, metadata) {
  const temporary = path.join(path.dirname(destination), `.worktree-inherit-${randomUUID()}`);
  const input = fs.openSync(source, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
  let output;
  try {
    output = fs.openSync(temporary, 'wx', 0o600);
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let count;
    while ((count = fs.readSync(input, buffer)) > 0) {
      let written = 0;
      while (written < count) written += fs.writeSync(output, buffer, written, count - written);
    }
    fs.fchmodSync(output, metadata.mode);
    try {
      fs.linkSync(temporary, destination);
      return 1;
    } catch (error) {
      if (error.code === 'EEXIST') return 0;
      throw error;
    }
  } finally {
    fs.closeSync(input);
    if (output !== undefined) {
      fs.closeSync(output);
      fs.unlinkSync(temporary);
    }
  }
}

function inherit(checkout) {
  if (checkout && (checkout.length !== 3 || !/^(?:0{40}|0{64})$/.test(checkout[0]) || checkout[2] !== '1')) return 0;
  let root;
  try {
    root = fs.realpathSync(git(process.cwd(), ['rev-parse', '--show-toplevel']).replace(/\r?\n$/, ''));
  } catch (error) {
    if (error.message.includes('not a git repository')) return 0;
    throw error;
  }
  const gitDirectory = git(root, ['rev-parse', '--path-format=absolute', '--git-dir']);
  if (gitDirectory === git(root, ['rev-parse', '--path-format=absolute', '--git-common-dir'])) return 0;
  const primary = git(root, ['worktree', 'list', '--porcelain', '-z']).split('\0')[0];
  if (!primary.startsWith('worktree ')) throw new Error('cannot locate primary worktree');
  let source;
  try {
    source = fs.realpathSync(primary.slice('worktree '.length));
  } catch (error) {
    throw new Error(`primary worktree unavailable: ${error.message}`);
  }
  let manifestPath = path.join(root, manifestName);
  let manifest = readConfiguration(manifestPath);
  if (manifest === null) {
    manifestPath = path.join(source, manifestName);
    manifest = readConfiguration(manifestPath);
  }
  if (manifest === null) throw new Error('missing .worktreeinclude; initialize the primary worktree shared configuration first');
  for (const line of manifest.split(/\r?\n/)) {
    if (line.replace(/^!/, '').split(/[\\/]/).includes('..')) throw new Error(`${manifestName} pattern escapes project: ${line}`);
  }
  const selected = git(source, ['ls-files', '--cached', '--others', '--ignored', `--exclude-from=${manifestPath}`, '-z']);
  let copied = 0;
  for (const relative of new Set(selected.split('\0').filter(Boolean))) {
    const components = relative.split('/');
    if (path.isAbsolute(relative) || components.some(component => component === '..' || component === '.')) throw new Error('local configuration path escapes project');
    if (components.some(component => excludedNames.has(component) || excludedSuffixes.some(suffix => component.endsWith(suffix)))) continue;
    const destination = path.join(root, relative);
    if (inspect(destination)) continue;
    if (!prepareParents(source, relative, false)) continue;
    const input = path.join(source, relative);
    const metadata = inspect(input);
    if (!metadata || metadata.isDirectory()) continue;
    if (!metadata.isFile()) throw new Error(`refusing non-file local configuration: ${input}`);
    prepareParents(root, relative, true);
    if (configurationIsShadowed(root, source, relative)) continue;
    copied += copyFile(input, destination, metadata);
  }
  return copied;
}

try {
  const args = process.argv.slice(2);
  const json = args[0] === '--json';
  if (json) args.shift();
  if (args.length && args[0] !== '--post-checkout') throw new Error('usage: node .codex/scripts/worktree-inherit.mjs [--post-checkout OLD_HEAD NEW_HEAD CHECKOUT_FLAG]');
  const checkout = args.length ? args.slice(1) : null;
  const copied = inherit(checkout);
  if (json) process.stdout.write(`${JSON.stringify({ copied })}\n`);
  else if (!checkout || copied > 0) process.stdout.write(`inherited ${copied} missing local configuration files\n`);
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
