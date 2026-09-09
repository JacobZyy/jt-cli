// Run against an installed Whistle package and outputs from the actual built CLI.
// Node is used because this package has no TypeScript script runtime.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';

const [whistlePackage, rulesFile, method, url, scenario = 'base', query = '__mock'] = process.argv.slice(2);
assert(whistlePackage && rulesFile && method && url, 'Usage: node whistle-mock.mjs <whistle package directory> <rules file> <method> <URL without query> [scenario] [query]');
const require = createRequire(import.meta.url);
const { Rules } = require(path.join(path.resolve(whistlePackage), 'lib/rules'));
const manager = new Rules();
manager.parse(fs.readFileSync(rulesFile, 'utf8'));
function resolve(suffix, requestMethod = method) {
  const fullUrl = url + suffix;
  return manager.resolveRules({ fullUrl, curUrl: fullUrl, method: requestMethod, headers: {} }).rule?.matcher;
}
const base = resolve('');
const selected = resolve(`?${query}=${scenario}`);
assert(base?.startsWith('file://'), 'Default must select a local file');
assert(selected?.startsWith('file://'), 'Known scenario must select a local file');
JSON.parse(fs.readFileSync(base.slice(7).replace(/^<|>$/g, ""), 'utf8'));
JSON.parse(fs.readFileSync(selected.slice(7).replace(/^<|>$/g, ""), 'utf8'));
for (const suffix of [`?a=1&${query}=${scenario}`, `?${query}=${scenario}&a=1`, `?a=1&${query}=${scenario}&b=2`]) {
  assert.equal(resolve(suffix), selected, suffix);
}
for (const suffix of [`?${query}=unknown`, `?${query}=${scenario}-extra`, `?${query}`, `?${query}=${scenario}&${query}=${scenario}`, `?${query}=unknown&${query}=${scenario}`]) {
  assert.equal(resolve(suffix), 'statusCode://400', suffix);
}
assert.equal(resolve(`?x${query}=${scenario}`), base);
assert.equal(resolve(`?q=${query}=${scenario}`), base);
assert.equal(resolve(`?q=?${query}=${scenario}`), base);
assert.equal(resolve(`?q=%E4%B8%AD%E6%96%87&${query}=${scenario}`), selected);
assert.equal(resolve(`?${query}=${scenario}`, method === 'POST' ? 'GET' : 'POST'), undefined);
assert.equal(resolve(`/extra?${query}=${scenario}`), undefined);
assert.equal(resolve(`?%${query.charCodeAt(0).toString(16)}${query.slice(1)}=${scenario}`), 'statusCode://400');
assert.equal(resolve(`?${query}=%${scenario.charCodeAt(0).toString(16)}${scenario.slice(1)}`), 'statusCode://400');
console.log('Whistle parser: default, scenario, method, exact path, parameter order, value boundaries, encoded-selector rejection and duplicate rejection passed');
// Whistle starts housekeeping timers when loaded as a library.
process.exit(0);
