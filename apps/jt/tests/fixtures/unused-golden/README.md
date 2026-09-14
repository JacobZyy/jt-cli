# `jt unused` golden fixture

This directory is a small project with deliberately known answers. `expected.json` is the contract checked by the CLI integration test.

Covered cases:

- direct calls are used;
- imports without reads remain unused symbols;
- export-only and self-recursive declarations remain unused;
- re-export-only declarations and files retain barrel evidence;
- real consumers through a barrel count as use;
- registry values count as references;
- write-only variables remain unused;
- test-only consumers do not count;
- same-name declarations stay distinct by path and position;
- entrypoint files are ignored, declaration files are type-consumer-only, and underscore-prefixed declarations remain candidates;
- dynamic module exports without an exact runtime target remain unknown.

The test removes Node.js from `PATH` so this fixture exercises the deterministic Oxc fallback. `../unused-semantic-golden` covers TypeScript/Volar and Vue template semantics when workspace Node.js dependencies are installed.
