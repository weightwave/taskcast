# Changesets Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the unreliable tag-based npm publish workflow with @changesets/cli, ensuring `workspace:*` references are always correctly resolved during publish.

**Architecture:** Install `@changesets/cli` at workspace root. Configure it for fixed versioning (all 9 packages share one version). Replace `npm.yml` with a changesets-based workflow that auto-creates a "Version Packages" PR on main push, and publishes on merge. All packages use `workspace:*` internally (unchanged), and changesets handles the `workspace:*` → real version replacement at publish time.

**Tech Stack:** @changesets/cli, changesets/action (GitHub Action), pnpm workspaces

---

### Task 1: Install @changesets/cli

**Files:**
- Modify: `package.json` (root)
- Create: `.changeset/config.json`
- Create: `.changeset/README.md`

**Step 1: Install the dependency**

```bash
pnpm add -Dw @changesets/cli
```

**Step 2: Initialize changesets**

```bash
pnpm changeset init
```

This creates `.changeset/config.json` and `.changeset/README.md`.

**Step 3: Configure for fixed versioning**

Edit `.changeset/config.json` to:

```json
{
  "$schema": "https://unpkg.com/@changesets/config@3.1.1/schema.json",
  "changelog": "@changesets/cli/changelog",
  "commit": false,
  "fixed": [
    [
      "@taskcast/core",
      "@taskcast/server",
      "@taskcast/server-sdk",
      "@taskcast/client",
      "@taskcast/react",
      "@taskcast/cli",
      "@taskcast/redis",
      "@taskcast/postgres",
      "@taskcast/sentry"
    ]
  ],
  "access": "public",
  "baseBranch": "main",
  "updateInternalDependencies": "patch",
  "ignore": []
}
```

Key settings:
- `fixed` — all 9 packages always bump to the same version together
- `access: "public"` — scoped packages need this for npm
- `updateInternalDependencies: "patch"` — when a dependency bumps, dependents bump too

**Step 4: Verify the config**

```bash
cat .changeset/config.json
```

Expected: the JSON above.

**Step 5: Commit**

```bash
git add package.json pnpm-lock.yaml .changeset/
git commit -m "infra: add @changesets/cli with fixed versioning config"
```

---

### Task 2: Add release script to root package.json

**Files:**
- Modify: `package.json` (root)

**Step 1: Add the ci:publish script**

Add to root `package.json` scripts:

```json
{
  "scripts": {
    "ci:publish": "pnpm build && pnpm publish -r --access public"
  }
}
```

**Step 2: Verify**

```bash
node -e "console.log(JSON.parse(require('fs').readFileSync('package.json','utf8')).scripts['ci:publish'])"
```

Expected: `pnpm build && pnpm publish -r --access public`

**Step 3: Commit**

```bash
git add package.json
git commit -m "infra: add ci:publish script for changesets"
```

---

### Task 3: Replace npm.yml with changesets workflow

**Files:**
- Delete: `.github/workflows/npm.yml`
- Create: `.github/workflows/release.yml`

**Step 1: Write the new workflow**

Create `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    branches:
      - main

permissions:
  contents: write
  pull-requests: write
  id-token: write

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}

jobs:
  release:
    runs-on: ubuntu-latest
    environment: npm
    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Setup pnpm
        uses: pnpm/action-setup@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: pnpm
          registry-url: https://registry.npmjs.org

      - name: Install dependencies
        run: pnpm install --frozen-lockfile

      - name: Build
        run: pnpm build

      - name: Type check
        run: pnpm lint

      - name: Test
        run: pnpm test

      - name: Create Release PR or Publish
        uses: changesets/action@v1
        with:
          publish: pnpm ci:publish
          commit: "chore: release packages"
          title: "chore: release packages"
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

How this works:
- When changesets exist in `.changeset/`: the action opens a "Version Packages" PR that bumps all versions and removes the changeset files
- When no changesets exist (i.e. the version PR was just merged): the action runs `pnpm ci:publish`, which builds and publishes. `pnpm publish -r` automatically replaces `workspace:*` with the real version from each package's `package.json`.

**Step 2: Delete the old workflow**

```bash
rm .github/workflows/npm.yml
```

**Step 3: Verify the new workflow is valid YAML**

```bash
node -e "require('js-yaml').load(require('fs').readFileSync('.github/workflows/release.yml','utf8')); console.log('valid')"
```

Expected: `valid`

**Step 4: Commit**

```bash
git add .github/workflows/release.yml
git rm .github/workflows/npm.yml
git commit -m "infra: replace tag-based npm publish with changesets workflow"
```

---

### Task 4: Create an initial changeset for first release

**Files:**
- Create: `.changeset/<random-name>.md`

**Step 1: Create a changeset to fix the broken 0.1.0 publish**

```bash
pnpm changeset
```

When prompted:
- Select **all packages**
- Choose **patch** bump (this will publish 0.1.1 to fix the broken 0.1.0)
- Summary: "fix: resolve workspace:* references in published packages"

Or create manually:

```bash
cat > .changeset/fix-workspace-refs.md << 'EOF'
---
"@taskcast/core": patch
"@taskcast/server": patch
"@taskcast/server-sdk": patch
"@taskcast/client": patch
"@taskcast/react": patch
"@taskcast/cli": patch
"@taskcast/redis": patch
"@taskcast/postgres": patch
"@taskcast/sentry": patch
---

fix: resolve workspace:\* references in published packages
EOF
```

**Step 2: Verify the changeset**

```bash
cat .changeset/fix-workspace-refs.md
```

Expected: all 9 packages listed with `patch`.

**Step 3: Commit**

```bash
git add .changeset/fix-workspace-refs.md
git commit -m "chore: add changeset for 0.1.1 release"
```

---

### Task 5: Verify locally that changesets version + publish works

**Step 1: Dry-run version bump**

```bash
pnpm changeset version
```

Expected: all 9 packages bump from `0.1.0` → `0.1.1`, `workspace:*` stays in source.

**Step 2: Verify versions were bumped**

```bash
grep '"version"' packages/*/package.json
```

Expected: all show `"version": "0.1.1"`.

**Step 3: Dry-run publish to check workspace:* resolution**

```bash
pnpm publish -r --access public --dry-run --no-git-checks 2>&1 | head -40
```

Expected: packages are listed for publish, no `workspace:*` errors.

**Step 4: Reset the version bump (don't actually commit it — CI will do this)**

```bash
git checkout -- packages/*/package.json packages/*/CHANGELOG.md package.json pnpm-lock.yaml
```

The changeset file `.changeset/fix-workspace-refs.md` should still exist (it was committed in Task 4).

**Step 5: Final sanity check**

```bash
ls .changeset/fix-workspace-refs.md && echo "Changeset present, ready for CI"
```

---

### Task 6: Update CLAUDE.md and documentation

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Add changesets info to the Commands section**

Add after the existing commands section:

```markdown
### Release Workflow

```bash
pnpm changeset          # Create a changeset (run before PR)
pnpm changeset version  # Bump versions (CI does this)
pnpm ci:publish         # Build + publish (CI does this)
```

All 9 packages use **fixed versioning** — every release bumps all packages to the same version.

When you merge a PR that contains `.changeset/*.md` files, CI will:
1. Open a "Release Packages" PR that bumps versions and generates changelogs
2. When that PR is merged, CI publishes to npm
```

**Step 2: Remove any references to tag-based publishing if present**

Search CLAUDE.md for references to `v*` tags or `npm.yml` and remove/update them.

**Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: document changesets release workflow"
```