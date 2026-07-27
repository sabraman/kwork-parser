# Russian Telegram Interface Implementation Plan

1. Add a small compile-time `text` module for Russian elapsed-time, Boolean,
   empty-state, and count formatting.
2. Replace fixed Telegram-visible English strings in startup, commands, status,
   connection/authentication warnings, and completion responses.
3. Translate fixed labels in inbox, order, statistics, and digest notifications;
   preserve Kwork-provided names, statuses, usernames, URLs, and identifiers.
4. Add focused unit tests for Russian time forms, plural forms, command help,
   status values, and existing authorization/command parsing.
5. Run formatting, tests, clippy, MSRV, ShellCheck, Actionlint, and a source
   audit for known English Telegram phrases.
6. Request a code/security review, address any Important findings, push `main`,
   and verify `/start` and `/status` from the administrator chat.
