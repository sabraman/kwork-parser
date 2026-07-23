# kwork-parser

A tiny, single-owner Kwork monitor that sends inbox, order, and seller-stat
notifications to Telegram. It is designed to run as one blocking Rust process
on a 128 MB VPS.

- [Frankenstein](https://github.com/ayrat555/frankenstein) with its blocking
  `ureq` client for Telegram
- Kwork's mobile JSON API instead of Chromium or HTML scraping
- One bounded JSON state file instead of SQLite or an embedded database
- No Tokio runtime, browser, webhook, web server, or inbound port

> [!IMPORTANT]
> This project uses an unofficial Kwork API. Kwork can change or disable that
> API without notice. Use conservative polling intervals and comply with
> Kwork's rules.

## Features

- New unread-dialog notifications with deduplication
- New-order and order-status notifications
- View and order deltas for each kwork, keyed by stable kwork ID
- Periodic account digest: balance, rating, connects, unread counts, and kworks
- Adaptive inbox polling and configurable quiet hours
- `/start`, `/inbox`, `/orders`, `/stats`, `/summary`, and `/status`
- A fixed administrator ID; every other Telegram chat is ignored
- Bounded HTTP responses, persistent state, messages, and error text
- Atomic, owner-only state and Kwork-token files
- Background Kwork reconnection with exponential backoff up to five minutes

## Requirements

- A Telegram bot created with [@BotFather](https://t.me/BotFather)
- Your numeric Telegram user ID
- A Kwork seller login and password
- Rust 1.88 or newer for local builds

If a bot token is ever posted publicly or shared in a chat, revoke it in
BotFather and create a replacement before deployment.

## Configure

```bash
cp .env.example .env
chmod 600 .env
```

Set the four required values:

```dotenv
TELEGRAM_BOT_TOKEN=replace_me
TELEGRAM_ADMIN_ID=123456789
KWORK_LOGIN=replace_me
KWORK_PASSWORD=replace_me
```

Why is `TELEGRAM_ADMIN_ID` necessary? Telegram's `sendMessage` API needs a
numeric destination. Bots cannot initiate a private chat by username or phone
number. Send the bot `/start` once, then it can deliver scheduled notifications
directly to this ID. The bot never auto-binds another user.

You can learn your ID from a bot such as `@userinfobot`, or send your bot a
message and inspect Telegram's `getUpdates` response. Do not put the bot token
in a URL that you share or save publicly.

## Build and run

```bash
cargo test --locked
cargo build --release --locked
./target/release/kwork-parser
```

Check Kwork credentials without starting Telegram polling or changing the bot
state:

```bash
cargo run --release --locked -- --check
```

The check uses `.kwork-token-check.json` by default. The normal service caches
its Kwork token in `.kwork-token.json`; both paths are ignored by git.

## Configuration reference

| Variable | Default | Meaning |
| --- | ---: | --- |
| `TELEGRAM_BOT_TOKEN` | required | Replacement BotFather token |
| `TELEGRAM_ADMIN_ID` | required | Only notification and command destination |
| `KWORK_LOGIN` | required | Kwork login |
| `KWORK_PASSWORD` | required | Kwork password |
| `MESSAGE_CHECK_INTERVAL` | `3` | Base inbox interval, minutes |
| `MESSAGE_CHECK_MIN` | `1` | Inbox interval after recent activity, minutes |
| `MESSAGE_CHECK_MAX` | `10` | Inbox interval after long inactivity, minutes |
| `STATS_CHECK_INTERVAL` | `60` | Kwork-stat interval, minutes |
| `ORDERS_CHECK_INTERVAL` | `5` | Order interval, minutes |
| `SUMMARY_INTERVAL_HOURS` | `6` | Digest interval, hours |
| `QUIET_HOURS` | empty | Local time window such as `22-8` |
| `QUIET_ALLOW_INBOX` | `true` | Deliver inbox alerts during quiet hours |
| `STATE_PATH` | `kwork-state.json` | Persistent state file |
| `KWORK_TOKEN_PATH` | `.kwork-token.json` | Cached Kwork API token |
| `RUST_LOG` | `info` | `off`, `error`, `warn`, `info`, `debug`, or `trace` |

The inbox interval settings must satisfy `MIN <= INTERVAL <= MAX`. Command
responses are never suppressed by quiet hours.

## Run on a 128 MB VPS

Compile somewhere with more memory. A Rust release build itself can exceed
128 MB even though the finished bot is small. Build on a compatible Linux
machine or in CI, then copy only the binary to the VPS.

On the VPS:

```bash
sudo useradd --system --home /var/lib/kwork-parser --shell /usr/sbin/nologin kwork-parser
sudo install -m 0755 target/release/kwork-parser /usr/local/bin/kwork-parser
sudo install -d -o kwork-parser -g kwork-parser -m 0700 /var/lib/kwork-parser
sudo install -m 0600 .env.example /etc/kwork-parser.env
sudo install -m 0644 deploy/kwork-parser.service /etc/systemd/system/kwork-parser.service
sudoedit /etc/kwork-parser.env
sudo systemctl daemon-reload
sudo systemctl enable --now kwork-parser
```

The VPS environment file should use absolute writable paths:

```dotenv
STATE_PATH=/var/lib/kwork-parser/state.json
KWORK_TOKEN_PATH=/var/lib/kwork-parser/kwork-token.json
```

Inspect the service:

```bash
systemctl status kwork-parser
journalctl -u kwork-parser -f
systemctl show kwork-parser -p MemoryCurrent -p MemoryPeak
```

The included unit applies `MemoryHigh=80M` and `MemoryMax=96M`, leaving host
headroom. A small swap file is sensible as emergency protection, but sustained
swapping means the VPS is undersized or a response/state bound has regressed.

No domain, reverse proxy, certificate, firewall opening, or Telegram webhook is
needed. The service only makes outbound HTTPS requests.

### Automatic GitHub deployment

Every successful push to `main` builds a static x86-64 Linux binary and deploys
it through the GitHub `production` environment. Formatting, tests, clippy, the
Rust 1.88 compatibility test, and the release build must all pass first.

Bootstrap a dedicated deployment account once on the VPS:

```bash
sudo apt-get update
sudo apt-get install --yes file
sudo useradd --system --create-home --home-dir /var/lib/kwork-deploy --shell /bin/bash kwork-deploy
sudo install -d -o kwork-deploy -g kwork-deploy -m 0700 /var/lib/kwork-deploy/uploads
sudo install -o root -g root -m 0755 deploy/deploy-kwork-parser /usr/local/sbin/deploy-kwork-parser
sudo visudo -cf deploy/kwork-parser.sudoers
sudo install -o root -g root -m 0440 deploy/kwork-parser.sudoers /etc/sudoers.d/kwork-parser-deploy
sudo install -d -o kwork-deploy -g kwork-deploy -m 0700 /var/lib/kwork-deploy/.ssh
```

Create a dedicated Ed25519 key locally. Put its public key in
`/var/lib/kwork-deploy/.ssh/authorized_keys` with the key options
`restrict`, and set that file to owner `kwork-deploy:kwork-deploy`, mode `0600`.
Do not reuse a personal or root SSH key.

Configure the GitHub environment named `production`:

| Type | Name | Value |
| --- | --- | --- |
| Variable | `VPS_HOST` | VPS hostname or address |
| Variable | `VPS_PORT` | SSH port |
| Variable | `VPS_USER` | `kwork-deploy` |
| Secret | `VPS_SSH_KEY` | Dedicated private key |
| Secret | `VPS_KNOWN_HOSTS` | Pinned `ssh-keyscan` output for the VPS and port |

The deployment account can upload only to its own state directory. Its sudo
rule permits only the root-owned deployment script, which independently checks
the staging path, ownership, checksum, commit ID, and executable format.
Application credentials never leave `/etc/kwork-parser.env`.

The installer preserves `/usr/local/bin/kwork-parser.previous`, replaces the
binary atomically, and watches systemd for 20 seconds. It automatically restores
the previous binary if the new process exits or restarts during that window.
Production deployments are serialized; an in-progress deployment is never
cancelled by a newer commit.

Rotate access by creating a new key, replacing `authorized_keys`, updating the
`VPS_SSH_KEY` environment secret, and verifying one deployment before deleting
the old private key.

### Manual upgrade and rollback

```bash
sudo systemctl stop kwork-parser
sudo cp /usr/local/bin/kwork-parser /usr/local/bin/kwork-parser.previous
sudo install -m 0755 target/release/kwork-parser /usr/local/bin/kwork-parser
sudo systemctl start kwork-parser
```

To roll back, stop the service, restore `kwork-parser.previous`, and start it.
Back up `/var/lib/kwork-parser/state.json` if preserving deduplication state
matters. Credentials and cached tokens should not be included in general-purpose
backups unless the backup is encrypted.

## Memory design

- One OS thread and sequential network calls
- One shared Rustls/`ureq` dependency stack
- 2 MiB maximum Kwork response body
- 35-second Telegram request deadline instead of Frankenstein's long default
- At most 20 Telegram updates requested per long-poll response
- Telegram messages split below Telegram's size limit
- At most 1,000 dialog states, 1,000 kwork snapshots, and 2,000 order states
- Release LTO, one codegen unit, stripped symbols, size optimization, and
  abort-on-panic

Run measurements against the release binary. Live peak RSS requires real Kwork
and Telegram credentials; do not present idle or failed-startup RSS as a live
polling measurement.

Local Apple Silicon/macOS release measurement on 2026-07-23:

- stripped binary: 2,499,888 bytes;
- credential-free failed-startup maximum RSS: 2,162,688 bytes.

This confirms a very small baseline only. A representative Linux VPS polling
cycle still needs to be measured with private live credentials using
`systemctl show kwork-parser -p MemoryPeak`.

## Security notes

- Never commit `.env`, state, token-cache, or log files.
- The Kwork password and both tokens live in process memory while the service is
  running; restrict VPS access accordingly.
- Rotate a Telegram token immediately if it is exposed.
- Only `TELEGRAM_ADMIN_ID` can trigger commands. Unauthorized chats receive no
  response.
- HTTP and parser errors omit response bodies and redact known credentials.

## Development

```bash
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

## Inspiration

Inspired by [tokyotokyo-dev/parser-kwork](https://github.com/tokyotokyo-dev/parser-kwork).
This is an independent Rust port focused on a much smaller runtime footprint.

## License

MIT
