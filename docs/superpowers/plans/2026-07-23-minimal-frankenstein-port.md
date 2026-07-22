# Minimal Frankenstein Port Implementation Plan

1. Replace `redb` with a versioned, bounded JSON state module. Add unit tests for
   round trips, pruning, invalid versions, health metadata, and atomic saves.
2. Change Telegram ownership to required `TELEGRAM_ADMIN_ID`. Remove persisted
   first-contact binding and test that only the configured ID produces commands.
3. Harden configuration, Kwork HTTP response limits, token persistence, error
   redaction, retry behavior, scheduling, quiet hours, and the explicit smoke
   check.
4. Remove obsolete HTML/cookie helpers and dependencies. Reconcile `Cargo.toml`
   and regenerate the committed lockfile.
5. Update `.env.example`, `.gitignore`, README, and add a hardened `systemd` unit
   and deployment instructions for a 128 MB VPS. Credit the inspiration repo.
6. Run formatting, tests, clippy, release build, dependency inspection, and
   secret scans. Measure binary size and local release RSS with documented
   limitations.
7. Review the implementation, fix findings, commit only intended files, create a
   new public GitHub repository with `gh`, and push the verified history.
