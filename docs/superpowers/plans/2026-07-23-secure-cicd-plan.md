# Secure CI/CD Implementation Plan

1. Add a root-owned deployment script that validates an upload directory,
   checksum, commit identifier, ownership, and Linux executable format before
   touching the installed binary.
2. Make installation atomic, preserve one previous binary, check systemd health
   across a fixed window, and restore the previous binary on failure.
3. Extend GitHub Actions with an x86_64 musl release artifact and a production
   deployment job gated by formatting, tests, clippy, and Rust 1.88 tests.
4. Pin every GitHub Action to an immutable commit and serialize production
   deployments without cancelling one in progress.
5. Add documented deploy-user, SSH-key, upload-directory, and sudoers setup.
6. Validate shell syntax, shellcheck, workflow structure, Rust checks, and a
   local Linux artifact build.
7. Provision the VPS and GitHub `production` environment, push to `main`, and
   verify one complete automatic deployment plus service memory and restart
   state.
