# @taskcast/cli

## 0.2.0

### Minor Changes

- ca5ec96: Add SQLite local storage adapter for zero-dependency development. Use `taskcast start --storage sqlite` to persist data locally without Redis or PostgreSQL.

### Patch Changes

- d4a391c: Unified release workflow: npm publish, Rust binary builds (5 platforms), and Docker image push now share a single version number and run in one workflow.
- Updated dependencies [ca5ec96]
- Updated dependencies [d4a391c]
  - @taskcast/sqlite@0.2.0
  - @taskcast/core@0.2.0
  - @taskcast/server@0.2.0
  - @taskcast/redis@0.2.0
  - @taskcast/postgres@0.2.0

## 0.1.2

### Patch Changes

- Updated dependencies [987c9df]
  - @taskcast/core@0.1.2
  - @taskcast/postgres@0.1.2
  - @taskcast/redis@0.1.2
  - @taskcast/server@0.1.2

## 0.1.1

### Patch Changes

- 5085c69: fix: resolve workspace:\* references in published packages
- Updated dependencies [5085c69]
  - @taskcast/core@0.1.1
  - @taskcast/server@0.1.1
  - @taskcast/redis@0.1.1
  - @taskcast/postgres@0.1.1
