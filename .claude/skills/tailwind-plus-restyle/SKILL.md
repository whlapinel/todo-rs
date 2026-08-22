---
name: tailwind-plus-restyle
description: How to fetch and use Tailwind Plus component source (from the user's Google Drive) when restyling a web_ui screen with a specific Tailwind Plus component.
---

# Tailwind Plus component library (`.vendor/tailwind-plus/`, gitignored)

The user has a Tailwind Plus subscription and downloads component source as zips from their Google Drive when a restyle needs a specific component (this is how the items/team-items/checklists/dashboard/teams/assigned-items Tailwind Plus restyle commits got their markup). `.vendor/tailwind-plus/application-ui-v4/` is the extracted `application-ui-v4.zip` (Tailwind Plus's "Application UI" category) — not committed (`.gitignore`'d, `/.vendor/`), so it must be re-fetched from Drive in a fresh clone or a session that hasn't pulled it down yet. Layout: `html/<category>/<component-group>/NN-variant-name.html` (plain HTML + Tailwind classes, no framework), with sibling `react/` and `vue/` trees for the same components — **always use the `html/` tree**, never `react/`/`vue/`, since this app has no browser-side framework (server-rendered Askama + htmx only). E.g. the month-view calendar used for the Events calendar view is `html/data-display/calendars/02-month-view.html`.

If `.vendor/tailwind-plus/` is missing, ask the user which Drive file to pull (they'll usually know, or it can be found by searching Drive for `application-ui-v4.zip` / similarly-named category zips) rather than assuming — there's no guarantee the same category zip covers whatever component is needed next.

**All the HTML components also live locally at `~/tailwind-ui-html/` (checked into GitHub under the same name, `tailwind-ui-html`)** — check there first before hunting elsewhere or re-fetching from Drive.
