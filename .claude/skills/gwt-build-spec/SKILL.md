---
name: gwt-build-spec
description: "Use when implementation should proceed from an approved SPEC or approved standalone task, and the work should run through the build/test/verify loop."
---

# gwt-build-spec

## Transition alias

`gwt-build-spec` is a temporary alias for `gwt-execute`. If this skill is
invoked with `SPEC-N`, continue as `$gwt-execute #N` and load
the matching `gwt-execute/SKILL.md` asset from the active provider skill tree.
This file exists only to route legacy invocations. All behavior lives in
`gwt-execute/SKILL.md`; do not duplicate it here.
