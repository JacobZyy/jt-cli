# Migrating to jt 4

jt 4 removes API contract generation and AI edit hooks from the jt distribution.
These capabilities are now maintained by the `nlab` CLI.

| Removed command | Replacement |
| --- | --- |
| `jt nlab-api` | `nlab api` |
| `jt ai-hook` | AI hook capabilities in `nlab` |

jt no longer ships the standalone nlab-api installer or the JT AI Hook Console.
Upgrading jt does not delete existing `.nlab` configuration, generated API files,
installed hook configuration/runtime files, or historical logs.
