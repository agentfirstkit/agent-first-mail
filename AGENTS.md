# AGENTS.md instructions

## Template data and rendering boundaries

- All generated Markdown and Review HTML must use the same template system and
  the same typed render documents. A Review context may contain or borrow those
  documents, but it must not independently rebuild, copy, flatten, alias, or
  approximate their mail, case, notification, draft, note, suggestion, or
  attachment data.
- Template input may contain canonical facts, relationships, operational
  interaction state, and output-independent domain analysis that requires mail
  or security knowledge. Examples of valid analysis include message direction,
  reply/forward classification, authentication alignment, possible-Bcc
  detection, message relationships, quote segmentation, and attachment type or
  fetch state.
- Model containment directly and keep each fact in one canonical place. Do not
  copy identifiers, messages, bodies, collection members, counts, timestamps,
  or status into parallel convenience fields when a consumer can traverse the
  contained document or derive the value from it. In particular, collection
  counts belong to templates as `items | length` or `messages | length`; they
  are not persisted or projected into render data.
- Persist domain intent explicitly when later state is insufficient to recover
  it. Never reconstruct a queued action, collection lifecycle, or relationship
  from a message's current status, a directory name, or generated output.
  Indexes and overlays must carry the exact canonical intent/status they read;
  they must not relabel it for presentation.
- Template input must not perform work that belongs to a template. Do not add
  localized or fallback copy, labels, symbols, CSS classes, layout rows,
  preformatted dates or byte sizes, output-specific escaping, display-only
  aliases, or pre-rendered Markdown/HTML such as a conversation string. The
  template and its output-specific filters own wording, localization,
  fallback selection, pluralization, formatting, escaping, markup, ordering,
  visibility, and layout.
- Keep render data output-neutral. Markdown and Review templates must select and
  format the same raw or semantic values rather than consuming separate
  Markdown-shaped and HTML-shaped projections.
- Workspace Review templates may provide MiniJinja, HTML, and CSS, but never
  JavaScript. afmail-owned trusted JavaScript may implement interaction
  behavior only; it must not construct rendered page structure or contain
  user-facing copy that belongs in language templates.
- Treat the typed template context as a versioned compatibility contract.
  Context changes must update every built-in language pack and include tests
  proving that Markdown and Review receive the same contained render documents,
  that every supported language is complete, and that template inputs contain
  no output-specific presentation fields.
- Render-cache identity must include the complete serialized typed document
  passed to the template. Never replace it with a hand-selected dependency map
  based on fields the current built-in template happens to read; workspace
  templates are free to consume any field in the contract.
- Review concurrency identity must hash every exact typed source document shown
  on that Review surface, excluding only the suggestion pair bound to that hash.
  Never hash a hand-selected projection of fields.
- Canonical editable notes must never be interpreted by comparing them with a
  rendered default or by recognizing template headings/markers. Built-in note
  scaffolds start empty; any scaffold output is real canonical note content.
