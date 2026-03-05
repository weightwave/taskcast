---
"@taskcast/cli": patch
---

Fix `npx @taskcast/cli` command (previously documented as `npx taskcast` which doesn't resolve) and fix global config creation race condition where `rl.close()` synchronously resolved the Promise to `false` before the user's answer was processed.
