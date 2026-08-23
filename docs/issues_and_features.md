# Issues & features

Open issues and feature requests, merged from the former `docs/issues.md` and `docs/features.md` (2026-08-20) into a single sorted list. Completed or superseded items — including ones from the old files that were already resolved but hadn't been moved out yet — live in `docs/archived/archived_issues_and_features.md`.


- Change main landing page to an all-projects page. This should have a sidebar with the same options as each project except each one includes all projects. So there's an all-projects calendar, an all-projects tasks list, an all-projects events list, etc... for background, we recently changed the main page from dashboard to calendar (not sure why I did that) but I think all-projects fits the purpose better. And default should be the list rather than the calendar. But it shouldn't be a calendar list - in fact, not sure why I have a list view of the calendar? Makes no sense. Let's remove it entirely.
- We should also have a new [item] action for each of these pages. For the all-projects view, Will need to add a project selector. I suppose the POST url will need to be built dynamically based on the project selected, since the url includes the project. instead of a separate page let's put the form in a dialog. we'll be applying this change to all per-project new-items and edit-items as well. No page/URL switching, just a dialog overlaid on the current page. I suppose this might be big enough to justify a multi-stage plan.
- Highlighted tab of day-drawer in calendar pages doesn't update with tab selection, stuck on 'All'.
- Calendar view day-drawer date element needs to be fixed-width so we don't have layout shift - the buttons on either side shift around when moving between dates. And let's shorten the string to e.g. Sat, 21 Aug so it fits on mobile more easily.
- Need filtering for all lists.
    - see ../tailwind-ui-html/e-commerce/category-filters.html for an example. needs to be given dark mode support and remove dependency on tailwind elements
    - Filter by: 
        - complete (default false), 
        - assigned to (default Me), 
        - project (if in all projects list) (default all),
        - due date (default = all, overdue, none)
        - schedule (default=all, scheduled in past, none)
        - recurring (default=true, no)
        - skipped (default false),
    - Sort by (hierarchical allowed):
        - due date
        - other options to be added later (maybe)

- Need a way to set user's time zone via web UI. There's currently no user config 
- A previous solution to the Google calendar import problem was that all-day events will be given a date of 12:00pm in order to avoid the shift to the previous day. The other option I was given is having a user-configurable time-zone. It didn't occur to me at the time of implementation but why not instead add the "all-day" concept to our event schema? Let's require events to either be marked all-day or have a scheduled start time.
- Allow copying a task or event to another project (distinct from the same-project "duplicate" action above).
- Add a `priority` field to task items and task series; sort first by priority, then by due date.
- Allow adding tags to items
- Consider visibly disabling inputs that will result in error. Specifically, recurring tasks that aren't current should have complete and skip grayed out, and skipped recurring tasks and events that aren't cursor should have unskip grayed out.
- Add metadata fields to `Item`: `created_at`, `created_by`, `deleted_at`, `deleted_by` (tag for deletion instead of deleting outright, allowing recovery), with a TTL after which the item is actually deleted. Depends on the soft-delete design question mentioned elsewhere. Add completed_by field to activity log.
    - Open design question: should delete mark a `deleted` column true instead of actually deleting, to allow undo within a timeframe and easy viewing of recently-deleted items? Unsure this is the right pattern — wants a tradeoffs discussion before deciding. If adopted, pairs with the metadata-fields item.
- Add reminder schema and UI to tasks, series, and events. 
- Default value is reminders pushed on the instant they're scheduled for (for tasks and events), and on the instant they're due (for tasks)
- Add in-app notifications for reminders.
- Create user settings schema and UI 
    - Configure notifications to toggle e-mail on/off (default = on)
    - Comment notifications configurability
        - radio with (default = all comments ) or only tasks I'm assigned, 
        - events (checkbox)
- Add comments for all items. Any team-member can comment on any item. No edit or delete for now.
- Add notifications for comments -- all team members notified of any comment by default
- Turn app into a PWA
- Add push notifications: need Apple for iOS and Google for Windows Desktop Chrome
- Infrastructure/dev-ops: need to automate deployment better. right now for todo for example, I run task docker-release, then cd ../home-server && task deploy, and I think one barrier to automation is I have a password for the prod server command. Ideally I'd like to set up a pipeline that activates on push, but don't necessarily want to use github actions, and want to consider switching to a local remote repo instead of github, maybe gitea?
- Big feature addition, needs fleshing out: user should be encouraged, but not necessarily restricted in any way, to give tasks both a due date and a schedule date. So some kind of gentle warning indicator can be clicked to take user to a page that lists all tasks with scheduled date and no due date, and all tasks with due date but no scheduled date. However -- contrary to part of what I just stated, Consider this restriction: requiring due date for all tasks, and making scheduled date optional (but encouraged through this warning). This would be a very big change and would involve a migration to make all tasks scheduled and lacking a due date to have a due date matching the scheduled date.
    - Future vision worth discussing now on this: I'm envisioning a system by which user can specify a due date and scheduling window for recurring tasks, allowing assignee to schedule completion of a task within the allowed window. But this is low-priority and only half-baked currently.
- Another big feature addition: project journals. Allow end-to-end encryption of journal for personal journal privacy. Low-priority and half-baked.
- Audit `src/storage/migrations/` for `CREATE INDEX` statements that run before the column-adding migration they depend on — only the `source_event_id` one is confirmed fixed; there may be others in the same wrong position.
- Contrast issue for the series "Children Template" select box: reported as white text on white background. The current markup (`templates/project_item_series/new_page.html`) already uses the standard dark-mode-aware select classes used everywhere else in the app, so this may already be fixed or may be browser/theme-specific — re-verify live in both themes before scoping any code change.
- Ctrl+click or tap+hold should select rows, allowing bulk actions: "delete-selected", "reschedule-selected", "assign-selected".
- Mass rescheduling ("skip current" without completing), across projects. The single-row version already shipped (`bd000b1`, "Finish the quick-reschedule dialog and extend it to Events") — see the archived doc. The mass/bulk version is fully designed in `docs/scheduled-catchup-plan.md` but not yet built: needs a new cross-project screen, a new repo method, and a bulk-action pattern this codebase doesn't have precedent for yet. **Before implementing:** that plan doc predates the item_series redesign and has zero series-awareness in its "overdue" query — add a `series_id IS NULL` guard (or equivalent) before implementing, or it will desync a stale materialized series occurrence from its cursor.
- Need a plan for deleting old data to keep the DB from growing infinitely (completed recurring/series occurrences, activity log rows), likely via a user-configurable retention/row-limit policy. No design doc exists yet — needs its own planning pass (retention shape, global vs. per-user config, delete vs. archive) before implementation.
- Should events have "save as template" ability? Not sure we need event templates, but perhaps — maybe templates should be called task templates to clarify the narrower purpose if we decide to rule out event templates. Needs a decision from the user, not sized as a task.
- Add a New-item button to the cross-project Home calendar (`templates/main_calendar/calendar_page.html`, `src/web_ui/main_calendar.rs` — renamed from `main_dashboard.rs`/`templates/main_dashboard/` in Stage 8 of `docs/calendar-day-drawer-plan.md`), mirroring the per-project calendar's `+ Task`/`+ Event` header buttons (`docs/calendar-day-drawer-plan.md` Stage 3). That plan's Confirmed design decisions section deliberately excluded this from Stage 4 ("item creation is inherently project-scoped and Home has no single natural project to create into") — the user has since said they did want one there and missed that decision during planning. Needs a design pass this plan never did: since Home isn't scoped to one project, the New button needs some way to pick which project the new item goes into (e.g. a project-picker dropdown before landing on `/tasks/new`/`/events/new`, or defaulting to the user's most-recently-active project) — not just a straight copy of the per-project version's two-link segmented control.
