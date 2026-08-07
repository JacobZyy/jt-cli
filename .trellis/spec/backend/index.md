# Backend Development Guidelines

> Current conventions for jt, a Rust command-line application. Trellis names this layer `backend`; jt has no server or database.

## Guidelines Index

| Guide | Scope | Status |
|-------|-------|--------|
| [Directory Structure](./directory-structure.md) | Command modules, feature submodules, assets, tests | Documented |
| [Persistence Guidelines](./database-guidelines.md) | Filesystem safety; explicit database absence | Documented |
| [Error Handling](./error-handling.md) | Typed errors, propagation, exit status | Documented |
| [Quality Guidelines](./quality-guidelines.md) | Rust gates, tests, safety review | Documented |
| [User Output and Logging](./logging-guidelines.md) | stdout/stderr, prompts, redaction | Documented |

## Usage

- Read the relevant guide before changing that boundary.
- Treat linked source files as examples of current behavior, not frozen APIs.
- Update a guide when reviewed code establishes a new convention.
- Record absent facilities as absent; do not invent server, database, or structured-logging rules.

Documentation language: English.
