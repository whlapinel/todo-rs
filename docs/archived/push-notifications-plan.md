# Push notifications for reminders

Status: **implemented, 2026-08-28**.

## Context

`docs/issues_and_features.md`'s first item asked for push notifications (iOS + Windows/Chrome). The PWA groundwork (`ba075d3`, service worker + manifest) landed specifically as "a prerequisite for push notifications (next item on the issues list)." The reminders feature (`docs/reminders-in-app-notifications-plan.md`) already had the full data model and delivery logic for *in-app* notifications (bell badge, poll, dismiss) but explicitly deferred push: "needs its own plan... a future push-delivery stage would want the same `list_due_for_user`-style query, run from a background sweep... instead of driven by a live poll."

This is that stage: the same `reminders` rows now get delivered via a real OS/browser push notification too, so a due reminder reaches the user even when the app isn't open. It reuses the existing `reminders` table and `sync_item_reminders` write path as-is — nothing about what gets reminded or when changed, only *how it's delivered* gained a second channel alongside the existing in-app one.

## Crate choice

`web-push-native` (pure Rust, RustCrypto-based), not the more established `web-push` crate — confirmed with the user before implementation. `web-push` pulls `openssl-sys` transitively via its `ece` dependency with no pure-Rust option, which risked breaking this repo's cross-compiling Dockerfile (builds on `$BUILDPLATFORM` via a cross-linker; reqwest is already deliberately rustls-only to avoid exactly this class of native-TLS dependency). `web-push-native` builds a plain `http::Request<Vec<u8>>` — no bundled HTTP client — so sending goes through the existing reqwest client (`src/push.rs::send_push`), with the request's method/uri/headers/body copied across by hand: `web-push-native` depends on `http` 1.x, reqwest 0.11 depends on `http` 0.2 — two different crate versions of the same-named type, no automatic conversion. The `http` 1.x dependency is renamed `http1` in `Cargo.toml` to avoid colliding with the crate-wide `http = "0.2"` (axum 0.6's version) that most of this codebase's `http::` references resolve against.

VAPID keys are a raw 32-byte P-256 private scalar (`ES256KeyPair::from_bytes`/`.to_bytes()`), base64url-encoded — not a PEM file, unlike most VAPID tutorials. `examples/gen_vapid_key.rs` (`cargo run --example gen_vapid_key`) generates one; the public key for the browser's `applicationServerKey` is derived from it at runtime (`PushConfig::from_env`, `src/push.rs`) — no separate public-key secret to manage.

## Data model

- `push_subscriptions` table (`id`, `user_id`, `endpoint` UNIQUE, `p256dh_key`, `auth_key`, `created_at`) — one row per device/browser a user has enabled push on. `PushSubscriptionRepo` (`src/storage/sqlite/mod.rs` trait, `src/storage/sqlite/push_subscriptions.rs` impl).
- `reminders.push_sent_at` — a second, independent "delivered" marker alongside the pre-existing `sent_at` (the in-app dismiss marker). Kept separate so the two channels don't interfere: dismissing in-app doesn't retroactively suppress a push that already fired, and vice versa. The only coupling: `ReminderRepo::list_due_for_push` (the sweep's query) also skips anything already dismissed in-app (`sent_at IS NOT NULL`).
- Both added by `src/storage/migrations/add_push_subscriptions.rs` (and the fresh-DB baseline in `create_pool()`, `src/storage/sqlite/mod.rs`).

## Config: `src/push.rs`

`PushConfig::from_env() -> Option<Self>` mirrors `EmailConfig::from_env` (`src/email.rs`) exactly — an optional feature, silently absent if `TODO_VAPID_PRIVATE_KEY`/`TODO_VAPID_SUBJECT` aren't set, no `Extension` wiring, called at the point of use (the sweep's startup gate in `src/main.rs`). `send_push` builds the encrypted, VAPID-signed request and sends it via reqwest, returning `PushSendOutcome::{Delivered, Gone, Failed}` — `Gone` (404/410) tells the sweep to prune that subscription.

## Background sweep: `service::push::sweep_due_reminders`

A second `tokio::spawn` loop in `src/main.rs`, alongside the pre-existing calendar-sync sweep (`docs/google-calendar-import-plan.md` Stage 5) — same shape, 60-second interval (reminders are minute-precision, unlike calendar sync's 15 minutes). Not spawned at all if `PushConfig::from_env()` returns `None`. Each tick: `reminders.list_due_for_push(now)` globally across every user, skip a completed/deleted item's stale reminder (marking it pushed so it stops being retried, mirroring `list_due_notifications_for_user`'s defensive skip), resolve the recipient's timezone (`users.timezone`, falling back to UTC when unset — most users don't have one set yet) for the notification text via the existing `reminder_labels` (`src/web_ui/mod.rs`), send to every subscription that user has, prune any that come back `Gone`, then mark the reminder pushed once regardless of per-subscription outcome (best-effort, matching the CSV import precedent).

## Subscribe/unsubscribe: `src/web_ui/push.rs`

Plain routes, no Smithy operation — a browser-native concept with no CLI/MCP equivalent, same precedent as `notifications.rs`. `GET /web/push/public-key` (plain text, 404 if unconfigured), `POST /web/push/subscribe` / `POST /web/push/unsubscribe` (`axum::Json`, driven by `fetch()` from `pwa-assets/push.js` rather than htmx — the first `Json` extractor in `web_ui`).

## Client side

- `pwa-assets/push.js` (copied into `static/` by the existing pwa-assets build step, no script changes needed): toggle enable/disable, `Notification.requestPermission()`, `pushManager.subscribe`, an iOS-not-installed guard (Web Push on iOS Safari only works from a home-screen-installed PWA — `window.matchMedia('(display-mode: standalone)')`).
- Toggle lives in the bell dropdown (`templates/notifications/list.html`), the one existing user-facing entry point for reminder notifications.
- `pwa-assets/sw.js` gained `push`/`notificationclick` listeners.

## Explicitly out of scope

- Any new notification type beyond what `reminders` already covers.
- The user-settings "email on/off" toggle — separate backlog item.
- Full IANA-timezone-aware push text for every user — most users have no `timezone` set yet (separate open backlog item); falls back to UTC.
