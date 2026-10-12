import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import { setupDependencies } from './dependencies.mjs';

const install = ['install', '--offline', '--no-audit', '--no-fund'];
const testEnvironment = { ...process.env };
delete testEnvironment.NODE_TEST_CONTEXT;

function fixture(t) {
  const directory = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'jt-codex-dependencies-')));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const main = path.join(directory, 'main');
  fs.mkdirSync(path.join(main, 'packages/fixture'), { recursive: true });
  fs.writeFileSync(path.join(main, '.npmrc'), 'audit=false\nfund=false\nregistry=https://registry.invalid/\n');
  fs.writeFileSync(path.join(main, 'package.json'), JSON.stringify({ name: 'codex-dependency-tests', private: true, type: 'module', dependencies: { 'codex-fixture': 'file:packages/fixture' }, scripts: { test: 'node --test check.test.mjs' } }));
  fs.writeFileSync(path.join(main, 'packages/fixture/package.json'), JSON.stringify({ name: 'codex-fixture', version: '1.0.0', type: 'module', exports: './index.mjs', bin: { 'codex-fixture': './bin.mjs' } }));
  fs.writeFileSync(path.join(main, 'packages/fixture/index.mjs'), 'export const meaning = 42;\n');
  fs.writeFileSync(path.join(main, 'packages/fixture/bin.mjs'), '#!/usr/bin/env node\nconsole.log(42);\n');
  fs.writeFileSync(path.join(main, 'check.test.mjs'), 'import assert from "node:assert/strict"; import { test } from "node:test"; import { meaning } from "codex-fixture"; test("installed dependency executes", () => assert.equal(meaning, 42));\n');
  let cow = true;
  const probe = path.join(directory, 'cow-probe');
  try { fs.copyFileSync(path.join(main, 'package.json'), probe, fs.constants.COPYFILE_FICLONE_FORCE); }
  catch {
    try {
      if (!['darwin', 'linux'].includes(process.platform)) throw new Error('native clone unavailable');
      execFileSync('/bin/cp', [process.platform === 'darwin' ? '-c' : '--reflink=always', path.join(main, 'package.json'), probe]);
    } catch { cow = false; }
  }
  fs.rmSync(probe, { force: true });
  const worktree = () => {
    const target = path.join(directory, `worktree-${fs.readdirSync(directory).length}`);
    fs.cpSync(main, target, { recursive: true, verbatimSymlinks: true, filter: source => path.basename(source) !== 'node_modules' });
    return target;
  };
  return { main, worktree, cow };
}

test('stamped local dependencies clone independently, execute tests, and preserve repeated setup', t => {
  const { main, worktree, cow } = fixture(t);
  assert.equal(setupDependencies(main, main, 'npm', install), 'installed');
  const target = worktree();
  assert.equal(setupDependencies(target, main, 'npm', install), cow ? 'reused' : 'installed');
  assert.match(execFileSync('npm', ['test'], { cwd: target, encoding: 'utf8', env: testEnvironment }), /pass 1/);
  const modules = path.join(target, 'node_modules');
  const inode = fs.statSync(modules).ino;
  assert.notEqual(inode, fs.statSync(path.join(main, 'node_modules')).ino);
  assert.equal(fs.readlinkSync(path.join(modules, 'codex-fixture')), fs.readlinkSync(path.join(main, 'node_modules/codex-fixture')));
  assert.equal(setupDependencies(target, main, 'npm', install), 'preserved');
  assert.equal(fs.statSync(modules).ino, inode);
  fs.writeFileSync(path.join(target, 'node_modules/.package-lock.json'), '{}\n');
  assert.notEqual(fs.readFileSync(path.join(main, 'node_modules/.package-lock.json'), 'utf8'), '{}\n');
  fs.writeFileSync(path.join(target, 'packages/fixture/index.mjs'), 'export const meaning = 43;\n');
  assert.equal(fs.readFileSync(path.join(main, 'packages/fixture/index.mjs'), 'utf8'), 'export const meaning = 42;\n');
});

test('changed lock/configuration/local sources, missing stamp, or incompatible runtime fall back to install', t => {
  const { main, worktree } = fixture(t);
  setupDependencies(main, main, 'npm', install);
  for (const change of [
    target => fs.appendFileSync(path.join(target, 'package-lock.json'), '\n'),
    target => fs.appendFileSync(path.join(target, '.npmrc'), 'save-exact=true\n'),
    target => fs.writeFileSync(path.join(target, 'packages/fixture/index.mjs'), 'export const meaning = 43;\n'),
    () => fs.rmSync(path.join(main, 'node_modules/.jt-codex-dependencies.json')),
    () => {
      const marker = path.join(main, 'node_modules/.jt-codex-dependencies.json');
      const stamp = JSON.parse(fs.readFileSync(marker, 'utf8'));
      stamp.fingerprint = 'different Node ABI or package manager version';
      fs.writeFileSync(marker, JSON.stringify(stamp));
    },
  ]) {
    const target = worktree();
    change(target);
    assert.equal(setupDependencies(target, main, 'npm', install), 'installed');
    assert.equal(setupDependencies(target, main, 'npm', install), 'preserved');
    setupDependencies(main, main, 'npm', install);
  }
  const existing = worktree();
  fs.mkdirSync(path.join(existing, 'node_modules'));
  assert.equal(setupDependencies(existing, main, 'npm', install), 'installed');
});

test('root install lifecycle runs normally; cloned launchers never retain main paths', t => {
  const { main, worktree, cow } = fixture(t);
  setupDependencies(main, main, 'npm', install);
  const launcher = path.join(main, 'node_modules/.bin/absolute-launcher');
  fs.writeFileSync(launcher, `#!/bin/sh\nNODE_PATH="${main}/node_modules"\n`);
  setupDependencies(main, main, 'npm', install);
  const target = worktree();
  assert.equal(setupDependencies(target, main, 'npm', install), cow ? 'reused' : 'installed');
  if (cow) {
    const content = fs.readFileSync(path.join(target, 'node_modules/.bin/absolute-launcher'), 'utf8');
    assert.ok(content.includes(`${target}/node_modules`));
    assert.ok(!content.includes(`${main}/node_modules`));
    assert.ok(fs.readFileSync(launcher, 'utf8').includes(`${main}/node_modules`));
  }
  const manifest = JSON.parse(fs.readFileSync(path.join(main, 'package.json'), 'utf8'));
  manifest.scripts.prepare = 'node prepare.mjs';
  fs.writeFileSync(path.join(main, 'package.json'), JSON.stringify(manifest));
  fs.writeFileSync(path.join(main, 'prepare.mjs'), 'import fs from "node:fs"; fs.writeFileSync("generated.txt", process.cwd());\n');
  setupDependencies(main, main, 'npm', install);
  const lifecycleTarget = worktree();
  fs.rmSync(path.join(lifecycleTarget, 'generated.txt'));
  assert.equal(setupDependencies(lifecycleTarget, main, 'npm', install), 'installed');
  assert.equal(fs.readFileSync(path.join(lifecycleTarget, 'generated.txt'), 'utf8'), lifecycleTarget);
  assert.ok(!fs.existsSync(path.join(lifecycleTarget, 'node_modules/.jt-codex-dependencies.json')));
  const ignoredPrepare = 'import fs from "node:fs"; fs.mkdirSync("dist", { recursive: true }); fs.writeFileSync("dist/required.txt", "prepared");\n';
  fs.writeFileSync(path.join(lifecycleTarget, 'prepare.mjs'), ignoredPrepare);
  setupDependencies(lifecycleTarget, main, 'npm', install);
  fs.rmSync(path.join(lifecycleTarget, 'dist'), { recursive: true });
  assert.equal(setupDependencies(lifecycleTarget, main, 'npm', install), 'installed');
  assert.equal(fs.readFileSync(path.join(lifecycleTarget, 'dist/required.txt'), 'utf8'), 'prepared');
});

test('shared and dangling node_modules symlinks are refused; external dependency links are not reused', t => {
  const { main, worktree } = fixture(t);
  setupDependencies(main, main, 'npm', install);
  for (const destination of [path.join(main, 'node_modules'), path.join(main, 'missing')]) {
    const target = worktree();
    fs.symlinkSync(destination, path.join(target, 'node_modules'));
    assert.throws(() => setupDependencies(target, main, 'npm', install), /refusing shared node_modules symlink/);
  }
  fs.symlinkSync(main, path.join(main, 'node_modules/absolute-source'));
  const target = worktree();
  assert.equal(setupDependencies(target, main, 'npm', install), 'installed');
  assert.ok(!fs.existsSync(path.join(target, 'node_modules/absolute-source')));
});

test('copy-on-write failure cleans partial copies and performs normal install', t => {
  const { main, worktree } = fixture(t);
  setupDependencies(main, main, 'npm', install);
  const target = worktree();
  const copy = fs.cpSync;
  fs.cpSync = (source, destination, options) => {
    if (source === path.join(main, 'node_modules')) {
      fs.mkdirSync(destination);
      throw new Error('copy-on-write is unsupported');
    }
    return copy(source, destination, options);
  };
  try { assert.equal(setupDependencies(target, main, 'npm', install), 'installed'); }
  finally { fs.cpSync = copy; }
  assert.ok(!fs.readdirSync(target).some(file => file.startsWith('node_modules.jt-codex-')));
  assert.match(execFileSync('npm', ['test'], { cwd: target, encoding: 'utf8', env: testEnvironment }), /pass 1/);
});

test('concurrent setup preserves owner lock; failed installer releases only its own lock', t => {
  const { main } = fixture(t);
  const lock = path.join(main, '.jt-codex-dependencies.lock');
  fs.mkdirSync(lock);
  fs.writeFileSync(path.join(lock, 'owner.json'), JSON.stringify({ pid: process.pid }));
  assert.throws(() => setupDependencies(main, main, 'npm', install), /dependency setup lock exists:.*PID/);
  assert.ok(fs.existsSync(path.join(lock, 'owner.json')));
  assert.ok(!fs.existsSync(path.join(main, 'node_modules')));
  fs.rmSync(lock, { recursive: true });
  const manifest = JSON.parse(fs.readFileSync(path.join(main, 'package.json'), 'utf8'));
  manifest.scripts.prepare = 'node -e "process.exit(7)"';
  fs.writeFileSync(path.join(main, 'package.json'), JSON.stringify(manifest));
  assert.throws(() => setupDependencies(main, main, 'npm', install), /npm install failed with exit status/);
  assert.ok(!fs.existsSync(lock));
  assert.ok(!fs.existsSync(path.join(main, 'node_modules/.jt-codex-dependencies.json')));
});

test('ignored local build outputs participate in compatibility; invalid native artifacts never get runtime stamp', t => {
  const { main, worktree } = fixture(t);
  fs.mkdirSync(path.join(main, 'packages/fixture/dist'));
  fs.writeFileSync(path.join(main, 'packages/fixture/dist/index.mjs'), 'export const meaning = 42;\n');
  const manifestPath = path.join(main, 'packages/fixture/package.json');
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  manifest.exports = './dist/index.mjs';
  fs.writeFileSync(manifestPath, JSON.stringify(manifest));
  fs.writeFileSync(path.join(main, '.gitignore'), 'node_modules/\ndist/\n');
  execFileSync('git', ['init', '--quiet', main]);
  execFileSync('git', ['add', '.'], { cwd: main });
  setupDependencies(main, main, 'npm', install);
  const target = worktree();
  fs.writeFileSync(path.join(target, 'packages/fixture/dist/index.mjs'), 'export const meaning = 43;\n');
  assert.equal(setupDependencies(target, main, 'npm', install), 'installed');
  fs.writeFileSync(path.join(main, 'packages/fixture/dist/broken.node'), 'not a native binary');
  assert.equal(setupDependencies(main, main, 'npm', install), 'installed');
  const other = worktree();
  assert.equal(setupDependencies(other, main, 'npm', install), 'installed');
});

test('pnpm file dependency layout and shell launcher reuse independently', t => {
  let version;
  try { version = execFileSync('pnpm', ['--version'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim(); }
  catch { t.skip('pnpm unavailable'); return; }
  const { main, worktree, cow } = fixture(t);
  const manifest = JSON.parse(fs.readFileSync(path.join(main, 'package.json'), 'utf8'));
  manifest.packageManager = `pnpm@${version}`;
  fs.writeFileSync(path.join(main, 'package.json'), JSON.stringify(manifest));
  const arguments_ = ['install', '--offline', '--ignore-scripts', '--config.manage-package-manager-versions=false'];
  assert.equal(setupDependencies(main, main, 'pnpm', arguments_), 'installed');
  const target = worktree();
  assert.equal(setupDependencies(target, main, 'pnpm', arguments_), cow ? 'reused' : 'installed');
  assert.match(execFileSync('pnpm', ['test'], { cwd: target, encoding: 'utf8', env: testEnvironment }), /pass 1/);
  if (cow) {
    const launcher = fs.readFileSync(path.join(target, 'node_modules/.bin/codex-fixture'), 'utf8');
    assert.ok(!launcher.includes(main));
    assert.equal(setupDependencies(target, main, 'pnpm', arguments_), 'preserved');
    const source = fs.realpathSync(path.join(main, 'node_modules/codex-fixture/index.mjs'));
    const copied = fs.realpathSync(path.join(target, 'node_modules/codex-fixture/index.mjs'));
    assert.notEqual(fs.statSync(source).ino, fs.statSync(copied).ino);
    fs.writeFileSync(copied, 'export const meaning = 43;\n');
    assert.equal(fs.readFileSync(source, 'utf8'), 'export const meaning = 42;\n');
  }
});

test('local package under ignored build directory uses normal install without claiming complete stamp', t => {
  const { main, worktree } = fixture(t);
  fs.mkdirSync(path.join(main, 'build'));
  fs.renameSync(path.join(main, 'packages/fixture'), path.join(main, 'build/fixture'));
  const manifest = JSON.parse(fs.readFileSync(path.join(main, 'package.json'), 'utf8'));
  manifest.dependencies['codex-fixture'] = 'file:build/fixture';
  fs.writeFileSync(path.join(main, 'package.json'), JSON.stringify(manifest));
  assert.equal(setupDependencies(main, main, 'npm', install), 'installed');
  assert.ok(!fs.existsSync(path.join(main, 'node_modules/.jt-codex-dependencies.json')));
  const target = worktree();
  assert.equal(setupDependencies(target, main, 'npm', install), 'installed');
  assert.match(execFileSync('npm', ['test'], { cwd: target, encoding: 'utf8', env: testEnvironment }), /pass 1/);
});
