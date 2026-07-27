# Russian Telegram Interface Design

## Objective

Make every administrator-facing Telegram message natural Russian while keeping
the bot's command names, lightweight architecture, saved state, and deployment
configuration compatible with the running service.

## Language Boundary

Russian is the only Telegram interface language. The commands remain:

- `/start`
- `/inbox`
- `/orders`
- `/stats`
- `/summary`
- `/status`

Their descriptions, progress responses, completion responses, notifications,
status fields, and warnings are Russian.

Technical text that is not sent to Telegram remains English:

- systemd and application logs;
- CLI `--check` output;
- environment-variable validation errors;
- persisted internal error records;
- source comments and developer documentation.

When an API error is useful to the administrator, a Russian explanation may be
followed by sanitized technical detail. Secrets must remain redacted by the
existing API error handling.

## Implementation Structure

Add a small compile-time text module for reusable Telegram constants and pure
formatters. This module must not add a runtime translation map, localization
framework, configuration variable, or external dependency.

Messages assembled around Kwork data remain near their domain modules, but all
of their fixed Telegram-visible labels become Russian. Shared formatting covers
elapsed time, Boolean values, connection state, and Russian count forms.

The approach should have no meaningful memory impact: static string literals
remain in the binary, and formatters allocate only the returned message strings
already required for Telegram delivery.

## Telegram Messages

The following message groups must be Russian:

- startup success and startup authentication failure;
- connection loss or restoration notices;
- `/start` command menu;
- `/inbox`, `/orders`, and `/stats` progress and completion responses;
- `/summary` and scheduled digest fallback labels;
- `/status` title, field labels, values, elapsed times, and empty values;
- inbox, order, statistics, and digest notifications;
- command-triggered and scheduled authentication warnings.

Existing concise emoji prefixes and Kwork links remain. Usernames, Kwork names,
order titles, order statuses returned by Kwork, paths, identifiers, and sanitized
technical error details are data and are not translated.

Unknown commands continue to receive no response. Unauthorized chats continue
to be ignored.

## Russian Formatting

Elapsed time uses compact Russian forms:

- `никогда`;
- `N сек. назад`;
- `N мин. назад`;
- `N ч. назад`;
- `N дн. назад`.

Boolean status values use `да` and `нет`. A missing Kwork connection uses
`нет подключения`. Unavailable optional Kwork data uses `недоступно`.

Where counts appear in prose, formatters use correct Russian forms, including
cases such as `1 уведомление`, `2 уведомления`, `5 уведомлений`, and the
corresponding order-event wording.

## Compatibility

No environment variable, Telegram token, administrator ID, Kwork credential,
state schema, command name, polling interval, quiet-hour rule, notification
deduplication behavior, or CI/CD security boundary changes.

The running state file is loaded without migration. Deployment uses the existing
automatic `main` pipeline and rollback process.

## Verification

Unit tests cover:

- Russian elapsed-time output;
- Russian plural selection across singular, paucal, plural, and teen values;
- Russian `/start` and `/status` output;
- connection and empty-state values;
- preservation of existing command parsing and authorization behavior.

A source/output audit checks known Telegram-facing English phrases have been
removed. Existing Rust, MSRV, clippy, formatting, shell, deployment, and release
checks must continue to pass.

After CI/CD succeeds, live verification sends `/start` and `/status` from the
administrator chat and confirms that the new deployed commit stays active with
zero restarts and one task.

## Out of Scope

- Multiple selectable languages
- Russian command aliases
- Translating VPS logs, CLI diagnostics, README prose, or Kwork-provided data
- Changing Telegram keyboards, webhooks, or BotFather metadata
- Changing notification schedules or adding new bot features
