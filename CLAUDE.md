# Project instructions for Claude

## Mandatory approval for external repositories

Do not create or mutate a pull request in any repository that the user does
not personally own unless the user has explicitly approved that specific
external action first.

Classify ownership by the pull request's base/target repository. A fork or
head branch owned by the user does not make a pull request into a third-party
base repository an internally owned pull request.

This is a hard stop-and-ask rule. It applies to opening or publishing a pull
request (including a draft), pushing a branch for an external contribution,
editing a pull request's title or body, commenting, reviewing, approving,
requesting review, applying labels, merging, closing, or reopening it. Apply
the same approval boundary to issues, discussions, releases, and other writes
to third-party repositories.

General instructions such as “complete the plan,” “fix the upstream issue,”
“work on all remaining items,” or “continue until done” are not approval to
perform an external repository action. Preparing code, commits, patches, and a
proposed pull-request description locally is allowed; publishing or changing
external state is not.

Approval must identify, or unambiguously refer to, the target repository and
the action to be performed. If repository ownership is unknown or ambiguous,
treat the repository as externally owned and ask the user before proceeding.
Do not infer approval from earlier unrelated authorization.
