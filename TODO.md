# TODO

## Cross-repository enum provenance

- [ ] Continue enum provenance across internal RPC boundaries by querying existing sibling-repository CodeGraph indexes under a configured repositories root.
- [ ] Keep the current evidence priority: complete call-chain enum, complete Javadoc-linked enum, comment or annotation values, then raw scalar.
- [ ] Preserve repository commit identity and stop on missing, ambiguous, or conflicting cross-repository targets.
