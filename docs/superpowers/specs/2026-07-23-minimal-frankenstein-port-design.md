# Design: Minimal Frankenstein Kwork Bot

**Date:** 2026-07-23  
**Status:** Approved and implemented
**Goal:** Replace the Python Kwork monitor with a very small Rust service using
[`frankenstein`](https://github.com/ayrat555/frankenstein), suitable for a VPS
with 128 MB RAM.

## 1. Scope

The service monitors one Kwork seller account and sends notifications to one
fixed Telegram administrator. It retains and improves the useful behavior of
the Python bot while removing Playwright, Chromium, Python, an async runtime,
and a database engine from the production runtime.

The supported runtime is the Rust binary. The old Python source may remain in
git as historical reference during the port, but deployment and documentation
must not require Python.

## 2. Success criteria

- The release process stays comfortably within a 128 MB VPS limit during normal
  polling and API responses.
- Telegram communication uses Frankenstein's blocking `ureq` client.
- No Tokio runtime, browser, web server, webhook, Docker daemon, SQLite, or
  embedded database is required.
- Inbox, order, and kwork-stat changes generate deduplicated notifications.
- Periodic digests and owner commands remain available.
- Only the configured Telegram administrator can receive notifications or run
  commands.
- Network and API failures do not terminate the main loop.
- Persistent state cannot grow without an explicit bound.
- The public repository and its history contain no credentials.

## 3. Architecture

The program is one blocking, single-threaded process:

```text
startup
  -> load and validate environment
  -> load compact state file
  -> authenticate with Kwork mobile API
  -> initialize Frankenstein bot
  -> run due Kwork jobs and Telegram long polling sequentially
  -> persist state atomically after changes
```

There is no concurrent scraping. Only one Telegram or Kwork response is handled
at a time, and response bodies are bounded before deserialization. This keeps
peak memory predictable and avoids executor, thread-stack, and duplicate-buffer
overhead.

### Components

- `config`: reads environment variables, validates numeric ranges, and holds no
  secrets beyond process memory.
- `bot`: long-polls Telegram, filters by administrator ID, parses commands, and
  sends plain-text notifications.
- `kwork`: calls the mobile JSON API, authenticates, refreshes the cached token,
  and exposes small typed results for dialogs, kworks, orders, and account data.
- `state`: owns bounded persistent state and performs atomic save operations.
- `jobs`: compares API results with state and produces notification text.
- `main`: schedules jobs with monotonic deadlines and applies retry backoff.

## 4. Telegram ownership and delivery

`TELEGRAM_ADMIN_ID` is required and is configured privately on the VPS. The
administrator's actual ID is not stored in the public repository or example
config.

Telegram requires a numeric chat ID for `sendMessage`; bots cannot initiate a
private conversation using a username or phone number. The administrator must
send the bot at least one message first. After that, scheduled notifications are
sent directly to `TELEGRAM_ADMIN_ID`.

There is no first-user binding or pairing flow. Updates from all other chat IDs
are ignored silently. Group messages are also ignored unless their chat ID is
explicitly configured as the administrator ID.

Supported commands:

- `/start`: concise help.
- `/inbox`: check messages immediately.
- `/orders`: check orders immediately.
- `/stats`: refresh kwork counters and show the summary.
- `/summary`: show the last saved kwork summary without a network request.
- `/status`: show last successful runs and the latest bounded error message.

Outbound messages use plain text. This avoids Markdown escaping, fallback
requests, and formatting-related failures.

## 5. Kwork access

The service uses the mobile JSON API at `api.kwork.ru` because it transfers and
parses less data than desktop HTML and avoids browser cookies and CSS selectors.
It authenticates with `KWORK_LOGIN` and `KWORK_PASSWORD`, then caches the API
token in a permission-restricted file.

The API is unofficial and may change. All endpoint details and wire types stay
inside the `kwork` module so a future API adjustment does not affect scheduling,
state, or Telegram code.

HTTP rules:

- One long-lived `ureq::Agent` per external service.
- Connect, read, and overall request timeouts.
- A strict response-size cap before JSON deserialization.
- At most one re-authentication attempt after an authentication failure.
- Exponential retry delay with an upper bound; no tight failure loop.
- Error messages exclude passwords, bot tokens, API tokens, and full response
  bodies.

## 6. Persistent state

State is a small Serde JSON document, loaded once and retained in memory:

```text
version
dialogs: user id -> last notification fingerprint
kworks: stable kwork id -> name, views, orders
orders: order id -> last status fingerprint
health: job name -> last success timestamp
last_error: bounded text and timestamp
```

The state file is written only after a state change. Saving writes a temporary
file in the same directory, flushes it, sets owner-only permissions on Unix, and
renames it over the destination. A format version allows explicit migrations.
An invalid file causes a clear startup error rather than silently losing
deduplication state.

Growth is bounded:

- Dialog state retains only the most recent fingerprint per dialog and is
  capped to the newest 1,000 dialogs.
- Kwork state retains only the current snapshot per stable kwork ID and is
  capped to 1,000 entries.
- Order state retains current/recent entries only and is capped to the newest
  2,000 orders.
- Error text is truncated to 500 Unicode scalar values.

These limits are deliberately far above a normal single account while
preventing unbounded memory and disk use.

## 7. Jobs and improvements

### Inbox

Poll dialogs, create a stable fingerprint from dialog ID, unread count, message
time, and a bounded preview, then notify only when an unread dialog's fingerprint
changes. Message previews are truncated before storage and delivery.

The normal interval is adaptive within configured minimum and maximum bounds:
recent activity uses the minimum, long inactivity uses the maximum, and other
periods use the base interval.

### Orders

Seed existing orders on the first successful run without notification. On later
runs, notify for a new order or changed status. Removed/old entries are pruned by
the state bound.

### Kwork stats

Compare current views and order counts with the last snapshot keyed by stable
kwork ID. Notify only for positive deltas. Renaming a kwork does not create a
false new entry.

### Digest and quiet hours

The periodic digest includes account balance, rating, connects, unread counts,
and the latest saved kwork summary. Configured quiet hours suppress scheduled
stats, orders, and digest notifications. Inbox alerts may remain enabled through
`QUIET_ALLOW_INBOX`.

Command responses are never suppressed by quiet hours.

### Health and recovery

Each successful job records a timestamp. `/status` reports these timestamps and
the latest sanitized error. Telegram and Kwork failures use bounded exponential
backoff. A failed job does not discard its previous state or stop unrelated
jobs.

## 8. Configuration

Required variables:

| Variable | Meaning |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | Replacement BotFather token |
| `TELEGRAM_ADMIN_ID` | Fixed notification and command chat ID |
| `KWORK_LOGIN` | Kwork login |
| `KWORK_PASSWORD` | Kwork password |

Optional variables have conservative defaults:

| Variable | Default | Meaning |
| --- | ---: | --- |
| `MESSAGE_CHECK_INTERVAL` | `3` | Base inbox interval, minutes |
| `MESSAGE_CHECK_MIN` | `1` | Active inbox interval, minutes |
| `MESSAGE_CHECK_MAX` | `10` | Idle inbox interval, minutes |
| `STATS_CHECK_INTERVAL` | `60` | Stats interval, minutes |
| `ORDERS_CHECK_INTERVAL` | `5` | Orders interval, minutes |
| `SUMMARY_INTERVAL_HOURS` | `6` | Digest interval, hours |
| `QUIET_HOURS` | empty | Local interval such as `22-8` |
| `QUIET_ALLOW_INBOX` | `true` | Deliver inbox alerts in quiet hours |
| `STATE_PATH` | `kwork-state.json` | Persistent state path |
| `KWORK_TOKEN_PATH` | `.kwork-token.json` | Cached Kwork token path |
| `RUST_LOG` | `info` | Log level |

The example environment file contains placeholders only. `.env`, token files,
state files, and logs are gitignored.

## 9. Memory and dependency budget

Production dependencies are limited to the functionality actually used:

- `frankenstein` with default features disabled and blocking `client-ureq`.
- One compatible `ureq` version and one TLS implementation.
- `serde` and `serde_json` for API and state documents.
- `base64` for the mobile API's required HTTP Basic authorization header.
- Minimal environment, logging, and signal-handling support.

The port removes `redb`, SQLite, HTML parsers, regex, Tokio, Reqwest, Playwright,
and Chromium. Release settings use LTO, one codegen unit, symbol stripping,
size optimization, and abort-on-panic.

Memory verification must use the release binary. The verification records:

- Binary size.
- Idle RSS after startup and a completed polling cycle.
- Peak RSS during representative inbox, stats, orders, and digest calls.
- Behavior under a service-level memory ceiling.

The acceptance ceiling is 96 MB for the bot process, leaving approximately
32 MB on a 128 MB host for the operating system and supervision. If realistic
live credentials are unavailable during development, local measurement is
reported as partial rather than presenting an estimate as measured fact.

## 10. Deployment

The preferred VPS is a minimal 64-bit Linux installation with swap available for
emergency protection. Deployment copies only the release binary and creates:

- a dedicated unprivileged system user;
- `/etc/kwork-bot.env`, mode `0600`, for credentials;
- `/var/lib/kwork-bot`, owned by the service user, for state and token cache;
- a hardened `systemd` service with automatic restart.

The unit sets `MemoryMax=96M`, `TasksMax=8`, a restart delay, a private temporary
directory, read-only system paths, and write access only to the state directory.
No inbound port, domain, TLS certificate, reverse proxy, or webhook is needed.

The README documents build-on-host and cross-build/copy deployment paths,
service installation, upgrades, logs, backup, rollback, and token rotation.

## 11. Repository and security

A new public GitHub repository is created with `gh` after local verification.
The README credits
[`tokyotokyo-dev/parser-kwork`](https://github.com/tokyotokyo-dev/parser-kwork)
as inspiration and credits Frankenstein as the Telegram library.

Before publishing, the complete git history and tracked files are scanned for
credential patterns. The Telegram token shared in conversation is considered
compromised and must be revoked in BotFather. It will not be used, stored, or
committed. Only a newly generated token may be placed in the private VPS
environment file.

Repository creation and push happen only after tests, linting, release build,
and secret scanning succeed.

## 12. Testing and acceptance

Unit tests cover:

- configuration parsing and interval validation;
- admin-only Telegram update filtering and command parsing;
- Kwork response deserialization using sanitized fixtures;
- dialog, order, and stat change detection;
- first-run seeding and duplicate suppression;
- state round-trip, bounds, pruning, invalid versions, and atomic replacement;
- quiet-hour behavior across midnight;
- token redaction and error truncation.

Offline tests do not call Telegram or Kwork. An optional explicit smoke command
checks Kwork connectivity without starting Telegram polling or mutating the
production state file.

Final acceptance requires:

1. `cargo test` passes.
2. `cargo clippy --all-targets -- -D warnings` passes.
3. The release build succeeds from the committed lockfile.
4. The README and example configuration agree with the implemented variables.
5. The public repository contains no secret or machine-specific path.
6. Measured memory results and any measurement limitations are documented.

## 13. Explicit non-goals

- Multiple Kwork accounts or multiple Telegram administrators.
- Replying to Kwork messages from Telegram.
- Telegram webhooks or an HTTP server.
- Browser automation, captcha solving, or HTML scraping fallback.
- Historical analytics beyond the latest snapshots needed for deltas.
- A container image as the primary 128 MB deployment method.
