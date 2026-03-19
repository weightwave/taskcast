---
"@taskcast/core": patch
---

fix: serialize per-task event publishing to prevent storage ordering races

When multiple events were published to the same task concurrently, async scheduling could cause events to be stored in a different order than their assigned indices, resulting in incorrect SSE history replay ordering. Added per-task emit serialization and terminal-state cleanup.
