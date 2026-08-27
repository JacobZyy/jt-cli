# `jt unused` semantic golden fixture

This project verifies behavior that requires the target project's TypeScript and Vue language tooling:

- Vue template component usage;
- Vue template variable and event-handler references;
- namespace import property resolution;
- exact distinction between used and unused exports from the same module.

The integration test runs when Node.js, `typescript`, `vue-tsc`, and `@vue/compiler-sfc` are installed. It prints an explicit skip reason in Cargo-only environments without those development dependencies.
