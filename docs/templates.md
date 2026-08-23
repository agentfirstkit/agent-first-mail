# Markdown and Review Templates

afmail uses one MiniJinja system for generated Markdown, new contact/draft/note
scaffolds, and Review HTML. Markdown and Review contain the same versioned,
typed render documents. Review does not build an HTML-specific copy of message,
collection, draft, notes, suggestion, or attachment data.

## Two Trees, One Override Point Each

Content — mail, drafts, notes, contacts, generated Markdown — is overridden in
the workspace `templates/` tree. The Review UI is overridden in an AFUI
frontend. Neither tree can serve the other's templates, so every template has
exactly one place it can be replaced from.

## Content Paths

Export the built-ins with:

```bash
afmail render templates
```

Language-specific entry templates live under the selected BCP 47 pack:

```text
templates/<language>/
  i18n/catalog.j2
  contact/default.md.j2
  message/section.md.j2
  triage/view.md.j2
  case/case.md.j2
  case/message.md.j2
  notification/notification.md.j2
  notification/message.md.j2
  status/index.md.j2
  status/message.md.j2
  draft/new.md.j2
  draft/reply.md.j2
  notes/default.md.j2
  notes/merge-section.md.j2
  workspace/DO_NOT_EDIT.txt.j2
```

Language-independent composition lives under:

```text
templates/shared/
  markdown/*.md.j2
```

Complete built-in packs ship for `en-US`, `de-DE`, `es-ES`, `fr-FR`, `ja-JP`,
`ko-KR`, `pt-BR`, `zh-CN`, and `zh-TW`.
`workspace.language_bcp47` selects the closest pack. Each locale catalog owns
translated copy, punctuation, singular/plural decisions, and other
language-specific macros.

A built-in language entry normally imports its catalog and includes a shared
layout. That is only the default composition. An override may replace the whole
entry, override a shared layout, add partials, hide or reorder facts, or use a
completely different Markdown/HTML structure. Additional includes resolve under
`<language>/` first and `shared/` second within the same tree; prefix an
include with `shared/` to select the shared tree explicitly.

Template paths must be ordinary files and directories inside their tree.
Absolute paths, `..`, backslash traversal, and symlinks are rejected. If a
selected override is invalid, afmail reports the error and never silently falls
back to the built-in version.

## Review UI Frontends

A custom Review UI is an AFUI frontend, keyed by afmail's provider id and the
`ui_kind` of the surface it replaces:

```text
.afui/frontends/afmail/<ui_kind>/
  frontend.json
  templates/
  assets/
```

`ui_kind` is one of `triage_review`, `case_review`, `notification_review`, or
`draft_review`. One frontend replaces one surface. The same layout under the
user's AFUI config directory installs a frontend for every workspace; the
workspace directory wins when both exist.

`frontend.json` declares the implementation and the contract version it was
written against:

```json
{
  "frontend_id": "my_mail_review",
  "ui_api_version": "2"
}
```

afmail implements Review UI API version `2`. That number covers the typed
Review render document, the template keys below, the trusted-runtime marker,
and the `data-afmail-*` and `data-copy-*` attributes the runtime reads. A
frontend declaring any other version is rejected.

Version 2 adds the open Review's bidirectional interaction declarations. The
built-in layout places the canonical Agent suggestion and the conversation in
one Agent card inside `data-afmail-review-content`. During a canonical
workspace refresh, the trusted runtime refreshes only that card's suggestion
subtree and moves the existing card into the new content, so UI-owned
user/Agent turns remain visible. The card declares the composer, input, channel
status, history, and inert turn templates with `data-afmail-agent-*` hooks. The
runtime clones only those template-declared turn structures and inserts message
text with `textContent`; it contains no user-facing copy or page structure of
its own. A replacement UI may arrange or omit visible history, but AFUI does
not persist or replay it.

When no canonical `agent_suggestion` exists, the built-in templates render only
the no-suggestion state, not a fallback menu of every possible disposition.
After an agent writes a suggestion or draft through an ordinary afmail command,
a controller response may request a reread; the page then rebuilds the review
content from canonical typed documents while preserving any UI-owned turns.

`templates/` inside a frontend has the same shape as the content tree, holding
only Review UI keys:

```text
templates/<language>/
  i18n/catalog.j2
  review/page.html.j2
  review/message.html.j2
  review/draft-editor.html.j2
  review/notes-editor.html.j2
templates/shared/
  review/page-layout.html.j2
  review/agent-workbench-layout.html.j2
  review/message-layout.html.j2
  review/draft-editor-layout.html.j2
  review/notes-editor-layout.html.j2
  review/style.css.j2
```

Every file is optional: anything a frontend does not supply comes from
afmail's built-in pack for the active language, so a frontend that replaces
only `review/page.html.j2` keeps localized messages and editors in all nine
languages. `afmail render templates` seeds every workspace frontend directory
that already exists with the built-in Review UI to start from; it never creates
a frontend and never overwrites files that are already there.

Create, inspect, trust, and compare frontends with `afui frontend`:

```bash
afui frontend init    --provider-id afmail --ui-kind case_review --frontend-id my_mail_review
afui frontend check   --provider-id afmail --ui-kind case_review
afui frontend enable  --provider-id afmail --ui-kind case_review
afui frontend disable --provider-id afmail --ui-kind case_review
```

A workspace frontend does nothing until `afui frontend enable` records its
exact contents. Editing any file in it revokes that trust, and afmail serves
its built-in UI again until the frontend is enabled anew — trust is a statement
about bytes, not about a directory. `afmail case review` reports the frontend
that is serving as `review_frontend_id` in its `review_ready` progress event,
and reports nothing there when the built-in UI is serving.

A frontend that cannot be loaded, or that declares a different
`ui_api_version`, fails the review command with `review_frontend_unreadable` or
`review_frontend_incompatible`. afmail does not quietly substitute its built-in
UI, because that is indistinguishable from the override having worked. Set
`AFUI_SAFE_MODE=1` to ignore every user frontend and open the built-in UI; safe
mode is honoured before any frontend file is read, so a frontend cannot break
the way out of itself. `afmail doctor` smoke-renders whichever Review UI would
actually serve each surface.

A frontend supplies presentation and nothing else. It cannot bypass typed
validation, the base-hash concurrency check, the workspace lock, or the
save/settle contract: those answers come from afmail's typed AFUI runtime calls
for a custom UI exactly as they do for the built-in one, so a Review page that
claims a save or a settlement succeeded has not made it so.

## Render Document Contract

The current render-document schema version is `3`; the Rust definitions live in
`src/render_document.rs`.

`MessageRenderDocument` contains exactly one canonical `MessageFile` under
`message`, plus output-independent semantic analysis and explicit content trust
and provenance. The body occurs only at `message.body_text`. Body analysis uses
UTF-8 byte offsets into that exact string:

```json
{
  "message": {
    "message_id": "message_...",
    "body_text": "Authored text\n\n> quoted text"
  },
  "analysis": {
    "direction": "inbound",
    "action": "reply",
    "body": {
      "segments": [
        {"kind": "authored", "start_bytes": 0, "end_bytes": 13},
        {"kind": "quoted", "start_bytes": 15, "end_bytes": 28}
      ]
    }
  }
}
```

`text_range` reads such a range and fails on an invalid or non-UTF-8 boundary.
An empty/whitespace body has no fabricated zero-length segment; templates choose
the localized empty-body presentation.

Containment is the only composition mechanism:

- `MessageContextDocument` contains one `message` and zero or more exact
  `related_messages`; it does not repeat their IDs in a parallel list.
- `TriageRenderDocument.context` is that exact message context. A reply-header
  case relationship is a typed fact at
  `context.message.message.case_relationship_suggestion`; generated Markdown is
  never read back as state.
- `CollectionRenderDocument` contains one canonical `MessageCollection` and
  exact `MessageRenderDocument` children. The collection persists membership
  only in `items`; it has no copied message/attachment counts or derived
  last-message timestamp. Templates use `items | length`, `messages | length`,
  and the contained message timestamps directly. Its optional `location`
  contains only the filesystem group, which is not present in the collection;
  lifecycle comes only from the canonical `collection.status`.
- `DraftRenderDocument.draft.frontmatter` is the exact `DraftFrontmatter`, and
  `draft.body_markdown` is the only body copy. `analysis.authored_body_range`,
  attachment existence/size, completeness, queue status, and `push_id` are
  semantic analysis only. Attachment analysis follows the order of the exact
  frontmatter paths instead of copying each path.
- `NotesMergeRenderDocument` contains exact primary and merged
  `NotesRenderDocument` values. Rust does not concatenate a pre-rendered merge
  section around the template. Case/notification notes `show` returns that same
  `NotesRenderDocument` instead of a second target/text projection.
- `DraftReplyRenderDocument` contains the exact parent
  `MessageRenderDocument`; the reply template selects and quotes its fields.
- `ContactRenderDocument` contains the exact `ContactFrontmatter`, raw notes,
  and one locator. Contact scaffolding and `contact show` use this same shape;
  the built-in scaffold emits only the raw notes value and therefore starts
  empty. A workspace scaffold may seed real canonical notes, but presentation
  placeholders are never parsed back or exempted from deletion checks.
- Templates that need no domain facts receive an explicit
  `EmptyRenderDocument`, not an anonymous map.

Built-in case, notification, and contact note bodies start empty. Deletion
checks inspect only whether the canonical raw body contains text; Rust never
renders a localized default again and compares it with stored notes. A
workspace `notes/default.md.j2` override that emits text intentionally seeds
canonical notes, so deleting that text requires the normal notes confirmation.

`afmail message show MESSAGE_ID` follows the same contract: canonical mail and
analysis are under `context.message`, related canonical documents are under
`context.related_messages`, paths are under `locator`, and the concurrency token
is `review_base_hash`. There is no compact/full switch and no duplicated
flattened message projection.

`afmail case show REF` and `afmail notification show REF` follow the same rule.
Their `context` is the exact `CollectionRenderDocument` passed to Markdown and
Review; only an outer locator and `review_base_hash` are added. They never embed
the generated Markdown text or rebuild a flattened collection projection.

Review hashes serialize the complete typed source document after removing only
the suggestion pair whose hash they bind. Message hashes cover the exact triage
context including related messages. Case hashes cover the exact collection,
contained messages, notes document, and active draft documents. Notification
hashes cover the exact collection, contained messages, and notes document. Hash
inputs are never hand-selected projections of fields.

Render data must remain output-neutral. Never add translated copy, fallback
labels, symbols, CSS classes, formatted dates or sizes, escaped strings,
display aliases, table rows, or pre-rendered Markdown/HTML. Templates and their
filters own wording, localization, fallback, pluralization, formatting,
escaping, markup, ordering, visibility, and layout.

Useful filters include `datetime`, `bytes`, `text_range`, `chronological`,
`notification_date_desc`, `message_item`, `message_status_key`, `display_name`,
`initials`, `asset_path`, and the Markdown filters `inline`, `plain`,
`markdown_code`, `markdown_alt`, `markdown_fence`, `markdown_quote`,
`markdown_table`, and `yaml`. The `bytes` filter uses `B`, `KiB`, and `MiB`.

## Review HTML and JavaScript

Review templates may contain MiniJinja, HTML, and CSS. They may not contain
JavaScript, inline event handlers, executable URL schemes, active embedded
content, disabled autoescaping, `safe` output (including filter blocks), or any
`data:` URL. afmail validates every frontend Review source, every included
partial, and the final composed HTML. Review forces HTML autoescaping
regardless of a partial's filename suffix.

afmail injects one trusted behavior-only runtime after server-side rendering.
A custom `review/page.html.j2` must contain exactly one marker where that runtime
belongs, normally immediately before `</body>`:

```html
<!-- afmail:trusted-runtime -->
```

The runtime handles autosave, per-item settlement, stale-base conflicts,
destructive confirmations, and draft tabs. There is no page-wide submit or
cancel — each item (a triage message, or the one case/notification a
case/notification review is about) settles on its own, and closing the window
is the ordinary end of every review. The runtime does not create visible
markup or carry user-facing copy. Templates supply localized runtime messages
through the `data-copy-*` attributes and opt controls into behavior with the
documented `data-afmail-*` hooks.

Raster attachment previews are read-only and limited to 16 MiB by default.
`review.max_inline_image_bytes` in `.afmail/config.json` controls the declared,
bounded-read, and decoded byte limit. An oversized attachment remains listed
with its canonical filename, content type, and size, but the typed image action
does not load or retain the bytes. Admitted previews use AFUI-managed blobs;
templates never construct an attachment URL.

Suggested actions are rendered fail-closed. Only the closed action enum gets
decision controls; an unknown action displays localized unsupported-state copy
and cannot fall through to a destructive action.

Review is served with a nonce-bound Content Security Policy. The template ban
still applies even when a browser would also block the content.

## Cache and Verification

`afmail render refresh` hashes the complete serialized typed document actually
passed to the entry template, the render-document and engine contract,
workspace language/timezone/default identity, and the complete built-in and
workspace language/shared template trees. The full-tree
hash covers workspace entries that include a different built-in entry or add
arbitrary partials. Rust does not select a smaller data projection based on
what the built-in template happens to read, so workspace templates may use any
field in the documented context without stale cache hits. A cache hit also
hashes the actual generated file, so hand-edited or corrupted output is
rebuilt.

Review is not cached; each request renders the page and re-resolves the
frontend, so a template edit and a revoked trust both take effect on the next
page load of a live session.

After editing templates:

1. Run `afmail render refresh` for generated Markdown.
2. Run `afmail doctor` to compile and smoke-render every template in the active
   language with typed fixture documents, including the Review UI that would
   actually serve each surface.
3. Open the relevant Review command to verify interactive layout changes.

Use `afmail render templates --force` only when intentionally replacing every
override — content and Review UI alike — with the current built-ins. Forcing
it over a trusted frontend changes those files, which revokes the frontend's
trust until `afui frontend enable` is run again.
