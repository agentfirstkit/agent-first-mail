# Authorization contract

afmail separates authorization by effect domain. An agent may use a narrower
operation when it is necessary to answer the user's request, but a broad or
remote operation is never an automatic prelude.

The local demo does not weaken these boundaries. Although its configured
remote is a loopback-only mock server, bare `afmail push` remains a preview and
only `afmail push --confirm PLAN_ID` applies that exact plan.

| Domain | Examples | Required user intent |
| --- | --- | --- |
| Local read | `status`, `message show`, bounded `list` | Implicit when needed to answer the current mailbox request. Prefer a supplied ref over broad discovery. |
| Remote read | `pull`, `remote folders`, `remote test` | The user asks to check, receive, sync, test, or otherwise use current remote mailbox state. Missing or stale local evidence also permits a pull after the agent explains why. |
| Local mutation | create a case, edit/queue a draft, mark local spam/trash, archive locally | The requested task entails organizing or preparing mail. Report that remote effects are only queued. Writing persistent notes requires an explicit request. |
| Remote mutation | `push --confirm PLAN_ID` | The user reviews the complete preview and authorizes that exact plan. A changed or expired plan requires a new review. |
| Permanent local deletion | `purge --confirm PLAN_ID` | The user reviews the purge preview and authorizes that exact plan. A private Git repository is a supported evidence/backup boundary, but is not deletion authorization. |

Invocation is automatic: supported agents may discover and load the installed
`agent-first-mail` skill when the user's request concerns mail. Automatic skill
selection does not authorize `pull`, `push`, `purge`, or any other operation
beyond the matrix above.

The HTTP API follows the same matrix. A valid bearer credential authenticates a
trusted client to one local afmail process; it does not grant remote-read intent,
authorize a push plan, authorize permanent deletion, or override an object's
review base hash. API callers must preserve the same user-intent checks as an
Agent using the CLI. Non-idempotent local HTTP mutations additionally require a
persistent `Idempotency-Key`, and remote push still requires the exact reviewed
`plan_id`.
