# Reduced-context review prompt

Use this as the Task `prompt` after an area PR is pushed. Fill every `{{placeholder}}`. Do not add planning history, other areas, or a defense of the patch.

The subagent has no parent conversation. If a field is unknown, write "unknown" rather than attaching extra context.

---

You are reviewing one stacked pull request. You did not write this code. You have no other areas of the broader plan.

## Contract (this area only)

{{area_spec_and_pseudocode}}

Out of scope (do not request this work in this PR):

{{out_of_scope}}

## How to inspect

- Repo: {{repo_path}}
- Base: `{{base_branch}}`
- Head: `{{head_branch}}`
- Changed files:

```
{{changed_files}}
```

Get the diff with:

```bash
git diff {{base_branch}}...HEAD
```

Read the changed files and the tests named in the contract. Do not explore unrelated packages.

## Review for

1. Correctness vs the contract and pseudo-code (missing paths, wrong signatures, inverted errors)
2. Scope creep (work that belongs in another area, or work the contract marked out of scope)
3. Tests that fail to prove the contract
4. Breakage at the boundary this area was supposed to stabilize
5. Safety, data loss, and obvious regressions in the touched surface

Ignore style nits that match existing code. Ignore features of later areas.

## Output

Return only:

```markdown
## Verdict
clean | needs-fix

## Must fix
- <file>: <finding> — <why it violates the contract or is a bug>

## Should fix
- <file>: <finding>

## Nit
- <file>: <finding>

## Follow-ups (later areas, not this PR)
- <item>
```

If there are no Must/Should items, verdict is `clean`. Do not propose a new architecture unless the contract is impossible as written; if so, say that under Must fix.
