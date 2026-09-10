// Validate one fixed response per interface with native Whistle mapping.
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
function resolve(suffix) {
  const fullUrl = url + suffix;
  return manager.resolveRules({ fullUrl, curUrl: fullUrl, method, headers: {} }).rule;
}
const base = resolve('');
const selected = resolve(`?${query}=${scenario}`);
for (const rule of [base, selected]) {
  assert(rule?.matcher.startsWith('file://'), 'Request must select a local file');
  assert.equal(rule.files.length, 1);
  assert(path.isAbsolute(rule.files[0]));
  JSON.parse(fs.readFileSync(rule.files[0], 'utf8'));
}
assert.equal(base.files[0], selected.files[0], 'Legacy state selectors must use the single response');
assert.equal(resolve(`?${query}=unknown`).files[0], base.files[0]);
assert.equal(resolve('/extra').files[0], base.files[0], 'A remaining path cannot change the fixed file target');
console.log('Whistle native mappings: single response, ignored state selector and absolute fixed file passed');
process.exit(0);
