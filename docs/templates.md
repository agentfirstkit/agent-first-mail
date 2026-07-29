# Markdown and Review Templates

afmail uses one MiniJinja system for generated Markdown, new contact/draft/note
scaffolds, and Review HTML. Markdown and Review contain the same versioned,
typed render documents. Review does not build an HTML-specific copy of message,
collection, draft, notes, suggestion, or attachment data.

## Workspace Paths

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
  review/page.html.j2
  review/message.html.j2
  review/draft-editor.html.j2
  review/notes-editor.html.j2
```

Language-independent composition lives under:

```text
templates/shared/
  markdown/*.md.j2
  review/*-layout.html.j2
  review/style.css.j2
```

Complete built-in packs ship for `en-US`, `de-DE`, `es-ES`, `fr-FR`, `ja-JP`,
`ko-KR`, `pt-BR`, `zh-CN`, and `zh-TW`.
`workspace.language_bcp47` selects the closest pack. Each locale catalog owns
translated copy, punctuation, singular/plural decisions, and other
language-specific macros.

A built-in language entry normally imports its catalog and includes a shared
layout. That is only the default composition. A workspace override may replace
the whole entry, override a shared layout, add partials, hide or reorder facts,
or use a completely different Markdown/HTML structure. Additional includes
resolve under `templates/<language>/` first and `templates/shared/` second;
prefix an include with `shared/` to select the shared tree explicitly.

Template paths must be ordinary files and directories inside `templates/`.
Absolute paths, `..`, backslash traversal, and symlinks are rejected. If a
selected override is invalid, afmail reports the error and never silently falls
back to the built-in version.

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

Workspace Review templates may contain MiniJinja, HTML, and CSS. They may not
contain JavaScript, inline event handlers, executable URL schemes, active
embedded content, disabled autoescaping, `safe` output (including filter
blocks), or any `data:` URL. afmail validates
workspace Review sources, every included partial, and the final composed HTML.
Review forces HTML autoescaping regardless of a partial's filename suffix.

afmail injects one trusted behavior-only runtime after server-side rendering.
A custom `review/page.html.j2` must contain exactly one marker where that runtime
belongs, normally immediately before `</body>`:

```html
<!-- afmail:trusted-runtime -->
```

The runtime handles autosave, submit/cancel, stale-base conflicts, destructive
confirmations, and draft tabs. It does not create visible markup or carry
user-facing copy. Templates supply localized runtime messages through the
`data-copy-*` attributes and opt controls into behavior with the documented
`data-afmail-*` hooks.

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

After editing templates:

1. Run `afmail render refresh` for generated Markdown.
2. Run `afmail doctor` to compile and smoke-render every template in the active
   language with typed fixture documents.
3. Open the relevant Review command to verify interactive layout changes.

Use `afmail render templates --force` only when intentionally replacing every
workspace override with the current built-ins.
