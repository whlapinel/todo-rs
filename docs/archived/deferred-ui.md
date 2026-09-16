# Deferred UI components

UI pieces that were removed from the templates (not from the backend) because
they broke layout, pending a redesign. Backend support is untouched and still
works (e.g. via direct API/CLI/MCP) — only the row-level UI trigger is gone.

## "Move under…" picker (removed 2026-08-11)

**Why removed:** the inline `<select>` in each row was too wide and its
dropdown had poor contrast (light background, light text) — badly broken on
mobile layouts. Needs a redesign (e.g. a modal or a dedicated action button
instead of an inline select) before it comes back.

**What still works:** the backend `POST /web/<screen>/<id>/subordinate`
routes (added in task #23, "promote-to-sibling and subordinate-to-sibling
item actions") are untouched — only the picker that triggered them from the
row was removed. Each row-rendering struct (`TaskRow`, `TeamTaskRow`,
`SimpleItemRow`, `TeamSimpleItemRow`) still computes and carries a `siblings:
Vec<(String, String)>` field (now unused by the template, hence a `dead_code`
warning on each — harmless, left in place so restoring the UI doesn't require
re-threading the data).

**Removed from these files** (`git log` / `git blame` around 2026-08-11 has
the exact prior diff if needed):
- `templates/tasks/row.html`
- `templates/team_tasks/row.html`
- `templates/simple_lists/row.html`
- `templates/team_simple_lists/row.html`

**Markup to restore** (personal tasks variant — team/simple variants swap the
URL prefix, e.g. `/web/team-tasks/{{ team_id }}/{{ id }}/subordinate`, and
tasks/team-tasks additionally gate on `!is_source_event_linked` since a
source-event-linked task can never be subordinated — see
`CLAUDE.md`'s Events section):

```html
{% if !complete && !siblings.is_empty() && !is_source_event_linked %}
<select name="new_parent_id" hx-post="/web/tasks/{{ id }}/subordinate" hx-trigger="change" hx-target="#page"
  hx-select="#page" hx-swap="innerHTML" hx-push-url="true"
  class="shrink-0 rounded-md border-0 bg-white py-1 pl-2 pr-6 text-xs text-gray-700 ring-1 ring-inset ring-gray-300 focus:ring-2 focus:ring-indigo-600 dark:bg-white/5 dark:text-gray-300 dark:ring-white/10">
  <option value="" selected disabled>Move under…</option>
  {% for s in siblings %}<option value="{{ s.0 }}">{{ s.1 }}</option>{% endfor %}
</select>
{% endif %}
```

`simple_lists.rs`/`team_simple_lists.rs` variants are the same shape minus
the `!complete`/`!is_source_event_linked` guards (Simple items have neither
concept):

```html
{% if !siblings.is_empty() %}
<select name="new_parent_id"
        hx-post="/web/simple-lists/{{ id }}/subordinate" hx-trigger="change"
        hx-target="#page" hx-select="#page" hx-swap="innerHTML" hx-push-url="true"
        class="shrink-0 rounded-md border-0 bg-white py-1 pl-2 pr-6 text-xs text-gray-700 ring-1 ring-inset ring-gray-300 focus:ring-2 focus:ring-indigo-600 dark:bg-white/5 dark:text-gray-300 dark:ring-white/10">
  <option value="" selected disabled>Move under…</option>
  {% for s in siblings %}<option value="{{ s.0 }}">{{ s.1 }}</option>{% endfor %}
</select>
{% endif %}
```

**Note for whoever restores this:** don't use `{{/* ... */}}` to comment
Askama templates — that's Go-template syntax, not Askama's. Askama's comment
syntax is `{# ... #}`. An earlier attempt to comment this block out with
`{{/* */}}` is what caused the build errors that led to this removal instead.
