---
name: gwt-fix-issue
description: "Use when the user wants to resolve an existing GitHub Issue by number or URL, especially when the workflow should continue through a direct fix unless a SPEC is needed."
---

# gwt-fix-issue

## Transition alias

`gwt-fix-issue` is a temporary alias for `gwt-execute`. If this skill is invoked
with `#N` or an Issue URL, continue as `$gwt-execute #N` and load
the matching `gwt-execute/SKILL.md` asset from the active provider skill tree.
This file exists only to route legacy invocations. All behavior lives in
`gwt-execute/SKILL.md`; do not duplicate it here.
