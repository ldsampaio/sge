---
schema_version: 1
open_count: 2
waived_count: 0
fixed_count: 0
total_count: 2
last_updated: 2026-10-13T14:18:15.791Z
---

# Broken Windows Ledger

> Cross-phase defect register. With `workflow.windows_enforce` enabled, `/gsd-ship` blocks while `open_count > 0`.
> Waive with `gsd-tools windows waive <id> "<reason>"` (reason required).
> Mark fixed with `gsd-tools windows fixed <id>`.

| id | phase | kind | file | line | description | status | reason | recorded_at | resolved_at |
|----|-------|------|------|------|-------------|--------|--------|-------------|-------------|
| 1 | 6 | deviation | .planning/config.json |  | git.allow_default_branch_commits=true added: pre-commit protected-branch assertion would block all task commits; project convention is GSD commits on main (solo local repo, mode yolo, all prior phases on main) | open |  | 2026-10-04T15:41:20.263Z |  |
| 2 | 21 | unrun-verify | scripts/live-21.sh |  | Phase 21-02 live gate not yet executed against the real server (human step with git-ignored .live-test.env creds); offline doubles green (328 lib + 19 bin + 42 safety), wrapper syntax-checked, exit-2 SKIP contract intact | open |  | 2026-10-13T14:18:15.791Z |  |

````json
[
  {
    "id": 1,
    "kind": "deviation",
    "phase": "6",
    "file": ".planning/config.json",
    "line": null,
    "description": "git.allow_default_branch_commits=true added: pre-commit protected-branch assertion would block all task commits; project convention is GSD commits on main (solo local repo, mode yolo, all prior phases on main)",
    "status": "open",
    "reason": "",
    "recorded_at": "2026-10-04T15:41:20.263Z",
    "resolved_at": null,
    "milestone": "v1.1"
  },
  {
    "id": 2,
    "kind": "unrun-verify",
    "phase": "21",
    "file": "scripts/live-21.sh",
    "line": null,
    "description": "Phase 21-02 live gate not yet executed against the real server (human step with git-ignored .live-test.env creds); offline doubles green (328 lib + 19 bin + 42 safety), wrapper syntax-checked, exit-2 SKIP contract intact",
    "status": "open",
    "reason": "",
    "recorded_at": "2026-10-13T14:18:15.791Z",
    "resolved_at": null,
    "milestone": "v1.4"
  }
]
````
