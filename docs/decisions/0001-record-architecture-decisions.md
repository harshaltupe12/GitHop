# 1. Record architecture decisions

Date: 2026-10-31 · Status: accepted

## Context

GitHop touches users' git and SSH setup. Decisions about what it may change, and how,
need a durable record so later changes don't silently break those guarantees.

## Decision

Record significant decisions as ADRs in `docs/decisions/NNNN-title.md` using this template:

```
# N. Title
Date: YYYY-MM-DD · Status: proposed | accepted | superseded by N
## Context
## Decision
## Consequences
```

Small decisions go in the decision log of the planning docs; an ADR is written when a
decision changes architecture, security or privacy guarantees.

## Consequences

Reviewers can check a change against the recorded reasoning instead of re-deriving it.
