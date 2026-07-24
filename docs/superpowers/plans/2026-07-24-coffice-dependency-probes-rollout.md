# Coffice Dependency Probe Rollout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Roll the released dependency-recovery Taskcast image through Coffice staging and production with HTTP startup, liveness, and dependency-aware readiness probes.

**Architecture:** This plan runs in the separate `aliyun-cn-gitops` repository after the Taskcast implementation plan has produced a published immutable image digest. Staging receives the image and probes first, undergoes controlled Redis/PostgreSQL fault tests with SLS verification, and only then is the same digest promoted to production.

**Tech Stack:** Kubernetes Deployments, Argo CD, Python/PyYAML validation, kubectl, Alibaba Cloud SLS, Coffice GitOps.

## Global Constraints

- Execute from `D:\Projects\weightwave\aliyun-cn-gitops`, not the Taskcast repository.
- Use one immutable Taskcast image digest that contains `/health/ready` and the approved reconnection behavior.
- Change staging before production.
- Keep `/health` for startup and liveness; use `/health/ready` only for readiness.
- Startup probe: period 2 seconds, timeout 2 seconds, failure threshold 30.
- Liveness probe: period 20 seconds, timeout 2 seconds, failure threshold 3.
- Readiness probe: period 5 seconds, timeout 2 seconds, failure threshold 2, success threshold 1.
- Do not modify or reveal SealedSecret ciphertext/plaintext.
- Do not restart Taskcast manually during controlled dependency fault tests.
- Use Alibaba Cloud profile `qisi-coffice`; every supported SLS operation must carry Coffice resource group `rg-aek6ht2o4f5oqtq`.
- Do not promote production unless the staging Pod UID/process remains unchanged through Redis and PostgreSQL recovery.

---

### Task 1: Stage the Immutable Image and HTTP Probes

**Files:**

- Modify: `apps/current/coffice-apps/coffice-staging/taskcast/workload.yaml`

**Interfaces:**

- Consumes `TASKCAST_IMAGE` and `TASKCAST_DIGEST`, copied exactly from the
  published Taskcast release.
- Produces the staging Deployment probe contract.

- [ ] **Step 1: Verify the release inputs before editing**

In PowerShell:

```powershell
if (-not $env:TASKCAST_IMAGE) { throw 'TASKCAST_IMAGE is required' }
if ($env:TASKCAST_DIGEST -notmatch '^sha256:[0-9a-f]{64}$') {
  throw 'TASKCAST_DIGEST must be an immutable sha256 digest'
}
```

Expected: no output and exit 0.

- [ ] **Step 2: Update only the staging image**

Run:

```powershell
python scripts/update-app-image.py `
  --root apps/current/coffice-apps `
  --namespace coffice-staging `
  --deployment taskcast `
  --container taskcast `
  --image $env:TASKCAST_IMAGE `
  --digest $env:TASKCAST_DIGEST
```

Expected: the staging Taskcast image is
`$env:TASKCAST_IMAGE@$env:TASKCAST_DIGEST`; production is unchanged.

- [ ] **Step 3: Replace staging TCP probes with exact HTTP probes**

In the Taskcast container, replace both existing probe blocks with:

```yaml
        startupProbe:
          httpGet:
            path: /health
            port: http
          timeoutSeconds: 2
          periodSeconds: 2
          failureThreshold: 30
        readinessProbe:
          httpGet:
            path: /health/ready
            port: http
          timeoutSeconds: 2
          periodSeconds: 5
          failureThreshold: 2
          successThreshold: 1
        livenessProbe:
          httpGet:
            path: /health
            port: http
          timeoutSeconds: 2
          periodSeconds: 20
          failureThreshold: 3
```

- [ ] **Step 4: Validate staging YAML and policy gates**

Run:

```powershell
python -c "import pathlib,yaml; list(yaml.safe_load_all(pathlib.Path('apps/current/coffice-apps/coffice-staging/taskcast/workload.yaml').read_text()))"
python scripts/check-no-plain-k8s-secrets.py
python scripts/check-no-sensitive-configmaps.py
python -m unittest discover -s scripts -p 'test_*.py'
git diff --check
```

Expected: every command exits 0.

- [ ] **Step 5: Commit staging**

```powershell
git add apps/current/coffice-apps/coffice-staging/taskcast/workload.yaml
git commit -m "deploy(staging): enable Taskcast dependency probes"
```

---

### Task 2: Verify Staging Dependency Recovery

**Files:**

- No repository files change.

**Interfaces:**

- Consumes the Argo CD-synchronized staging Deployment.
- Produces rollout evidence required for production promotion.

- [ ] **Step 1: Wait for Argo CD and Deployment health**

Run:

```powershell
kubectl --context qisi-infra -n coffice-staging rollout status deployment/taskcast --timeout=5m
kubectl --context qisi-infra -n coffice-staging get pod -l app.kubernetes.io/name=taskcast -o wide
```

Expected: rollout succeeds with one Ready Pod.

- [ ] **Step 2: Capture the stable Pod identity**

```powershell
$stagingPod = kubectl --context qisi-infra -n coffice-staging get pod -l app.kubernetes.io/name=taskcast -o jsonpath='{.items[0].metadata.name}'
$stagingUid = kubectl --context qisi-infra -n coffice-staging get pod $stagingPod -o jsonpath='{.metadata.uid}'
$stagingStart = kubectl --context qisi-infra -n coffice-staging get pod $stagingPod -o jsonpath='{.status.containerStatuses[0].state.running.startedAt}'
```

Expected: all three variables are non-empty.

- [ ] **Step 3: Verify normal health**

Port-forward in a dedicated terminal:

```powershell
kubectl --context qisi-infra -n coffice-staging port-forward deployment/taskcast 13721:3721
```

Then:

```powershell
Invoke-RestMethod http://127.0.0.1:13721/health
Invoke-RestMethod http://127.0.0.1:13721/health/ready
Invoke-RestMethod http://127.0.0.1:13721/health/detail
```

Expected: all report `ok: true`; detail lists only configured dependencies.

- [ ] **Step 4: Perform the approved controlled Redis restart**

First prove the exact staging instance belongs to the Coffice resource group:

```powershell
$redisInstance = 'r-uf6s2e4jdwcrdqr817'
$redisInventory = aliyun r-kvstore DescribeInstances `
  --profile qisi-coffice `
  --RegionId cn-shanghai `
  --ResourceGroupId rg-aek6ht2o4f5oqtq `
  --InstanceIds $redisInstance | ConvertFrom-Json
if (-not (($redisInventory.Instances.KVStoreInstance.InstanceId) -contains $redisInstance)) {
  throw 'staging Redis instance is not visible in the Coffice resource group'
}
```

Pause here for explicit operator approval of the staging Redis restart. After
approval, run exactly once and explicitly prevent a minor-version upgrade:

```powershell
aliyun r-kvstore RestartInstance `
  --profile qisi-coffice `
  --InstanceId $redisInstance `
  --EffectiveTime Immediately `
  --UpgradeMinorVersion false
```

During the restart, poll:

```powershell
kubectl --context qisi-infra -n coffice-staging get pod $stagingPod -w
```

Expected:

- `/health` remains 200;
- `/health/ready` changes to 503 and then returns to 200;
- the Taskcast Pod is temporarily Not Ready but is not restarted;
- cross-instance/event operations succeed after recovery;
- no interrupted business command is observed twice.

- [ ] **Step 5: Perform the approved controlled PostgreSQL restart**

First prove the exact staging instance belongs to the Coffice resource group:

```powershell
$postgresInstance = 'pgm-uf6loun06n04za0j'
$postgresInventory = aliyun rds DescribeDBInstances `
  --profile qisi-coffice `
  --RegionId cn-shanghai `
  --ResourceGroupId rg-aek6ht2o4f5oqtq `
  --DBInstanceId $postgresInstance | ConvertFrom-Json
if (-not (($postgresInventory.Items.DBInstance.DBInstanceId) -contains $postgresInstance)) {
  throw 'staging PostgreSQL instance is not visible in the Coffice resource group'
}
```

Pause here for explicit operator approval of the staging RDS restart. After
approval, generate one idempotency token and run exactly once:

```powershell
$rdsRestartToken = [Guid]::NewGuid().ToString('N')
aliyun rds RestartDBInstance `
  --profile qisi-coffice `
  --DBInstanceId $postgresInstance `
  --ClientToken $rdsRestartToken
```

Repeat the liveness/readiness and subsequent-operation checks.

Expected: the current SQL operation may fail once, readiness recovers, and
Taskcast itself is not restarted.

- [ ] **Step 6: Prove Pod/process identity stayed stable**

```powershell
$uidAfter = kubectl --context qisi-infra -n coffice-staging get pod $stagingPod -o jsonpath='{.metadata.uid}'
$startAfter = kubectl --context qisi-infra -n coffice-staging get pod $stagingPod -o jsonpath='{.status.containerStatuses[0].state.running.startedAt}'
if ($uidAfter -ne $stagingUid) { throw 'Taskcast Pod was replaced' }
if ($startAfter -ne $stagingStart) { throw 'Taskcast container restarted' }
```

Expected: no output and exit 0.

- [ ] **Step 7: Verify SLS transition records with resource-group scope**

Confirm the project through the resource-group-scoped list, then query the
staging Logstore:

```powershell
aliyun sls ListProject `
  --profile qisi-coffice `
  --region cn-shanghai `
  --resourceGroupId rg-aek6ht2o4f5oqtq

$slsTo = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() + 60
$slsFrom = $slsTo - 1800
aliyun sls GetLogs `
  --profile qisi-coffice `
  --region cn-shanghai `
  --project coffice-k3s-qisi-logs `
  --logstore coffice-staging `
  --from $slsFrom `
  --to $slsTo `
  --query 'event:dependency_state_change' `
  --line 100
```

Expected: sanitized Redis and PostgreSQL degraded/recovered records exist,
recovery records contain `downtimeMs`, and no connection string or credential
appears.

---

### Task 3: Promote the Proven Digest and Probes to Production

**Files:**

- Modify: `apps/current/coffice-apps/coffice-prod/taskcast/workload.yaml`

**Interfaces:**

- Consumes the exact staging-proven image digest.
- Produces the production Deployment probe contract.

- [ ] **Step 1: Update the production image to the same digest**

```powershell
python scripts/update-app-image.py `
  --root apps/current/coffice-apps `
  --namespace coffice-prod `
  --deployment taskcast `
  --container taskcast `
  --image $env:TASKCAST_IMAGE `
  --digest $env:TASKCAST_DIGEST
```

- [ ] **Step 2: Apply the exact staging probe block to production**

Copy the `startupProbe`, `readinessProbe`, and `livenessProbe` mappings from
the now-validated staging workload without changing any threshold.

- [ ] **Step 3: Verify staging and production probe parity**

```powershell
python -c "import yaml,pathlib; s=list(yaml.safe_load_all(pathlib.Path('apps/current/coffice-apps/coffice-staging/taskcast/workload.yaml').read_text()))[0]['spec']['template']['spec']['containers'][0]; p=list(yaml.safe_load_all(pathlib.Path('apps/current/coffice-apps/coffice-prod/taskcast/workload.yaml').read_text()))[0]['spec']['template']['spec']['containers'][0]; assert (s['startupProbe'],s['readinessProbe'],s['livenessProbe']) == (p['startupProbe'],p['readinessProbe'],p['livenessProbe']); assert s['image'] == p['image']"
```

Expected: exit 0.

- [ ] **Step 4: Run the complete GitOps validation gate**

```powershell
python -c "from pathlib import Path; import yaml; [list(yaml.safe_load_all(p.read_text())) for p in Path('.').rglob('*.yaml') if '.git' not in p.parts]"
helm lint vendor/loongcollector-3.2.6 -f vendor/loongcollector-3.2.6/values-coffice.yaml --strict
python scripts/check-no-plain-k8s-secrets.py
python scripts/check-no-sensitive-configmaps.py
python -m unittest discover -s scripts -p 'test_*.py'
git diff --check
```

Expected: every command exits 0.

- [ ] **Step 5: Commit production promotion**

```powershell
git add apps/current/coffice-apps/coffice-prod/taskcast/workload.yaml
git commit -m "deploy(prod): enable Taskcast dependency probes"
```

- [ ] **Step 6: Verify production rollout**

```powershell
kubectl --context qisi-infra -n coffice-prod rollout status deployment/taskcast --timeout=5m
kubectl --context qisi-infra -n coffice-prod get pod -l app.kubernetes.io/name=taskcast -o wide
```

Expected: one Ready Pod on the immutable staging-proven digest. Do not perform
an unplanned production dependency restart; rely on normal operation and the
next approved maintenance event for a production fault drill.

- [ ] **Step 7: Final repository verification**

```powershell
git status --short --branch
git diff --check HEAD~2 HEAD
git log --oneline --decorate -5
```

Expected: worktree clean with one staging and one production rollout commit.
