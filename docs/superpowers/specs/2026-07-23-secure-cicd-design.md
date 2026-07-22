# Secure CI/CD Design

## Objective

Deploy every successful push to `main` to the production VPS without installing
Rust on the server, exposing runtime credentials to GitHub, or granting GitHub
Actions unrestricted root access. A failed release must automatically restore
the last working binary.

## Existing System

- GitHub Actions currently checks formatting, tests, clippy, Rust 1.88
  compatibility, and a native release build.
- Production is Ubuntu 22.04 on x86_64 and runs `kwork-parser` under systemd.
- The service binary is `/usr/local/bin/kwork-parser`.
- Runtime credentials remain in `/etc/kwork-parser.env` and are not part of a
  deployment artifact.
- The application state is under `/var/lib/kwork-parser` and must survive
  releases and rollbacks.

## Architecture

GitHub-hosted runners build a statically linked
`x86_64-unknown-linux-musl` binary. Pull requests and all pushes run the quality
checks. A deployment job runs only for a successful push to `main` and uses the
GitHub `production` environment.

The VPS has a dedicated `kwork-deploy` account with key-only SSH access. GitHub
Actions connects as that account and may invoke one root-owned deployment
script through a narrowly scoped passwordless sudo rule. It does not receive a
root password and cannot read `/etc/kwork-parser.env`.

GitHub stores only these production secrets or variables:

- VPS host and SSH port
- deployment username
- private deployment key
- pinned SSH host public key or complete `known_hosts` entry

Telegram and Kwork credentials remain exclusively on the VPS.

## CI Workflow

For pull requests and pushes:

1. Check formatting with `cargo fmt`.
2. Run tests with the stable toolchain.
3. Run clippy with warnings denied.
4. Run tests using Rust 1.88, the minimum supported version.
5. Build the optimized Linux musl artifact on an Ubuntu runner.
6. Generate a SHA-256 checksum for the binary.

Actions are pinned to immutable commit SHAs. Cargo runs with `--locked`.

## CD Workflow

The deployment job depends on all required CI jobs and runs only when the event
is a push to `main`. GitHub environment name `production` provides a distinct
security boundary and deployment history. Repository concurrency permits only
one production deployment at a time; newer commits wait instead of interrupting
an active deployment.

The job:

1. Creates a temporary `known_hosts` file from the pinned production host key.
2. Loads the dedicated private key without printing it.
3. Uploads the binary and checksum to a unique staging directory owned by
   `kwork-deploy`.
4. Calls the permitted root deployment script with that staging directory.
5. Records the deployed commit, service state, restart count, and memory usage
   in the Actions log.
6. Removes runner and VPS staging data even if deployment fails.

No workflow step prints or transfers the runtime environment file.

## VPS Deployment Script

The root-owned script accepts one staging-directory argument and rejects paths
outside the dedicated upload directory. It then:

1. Verifies ownership, regular-file types, the SHA-256 checksum, and that the
   artifact is an executable x86-64 ELF binary.
2. Copies the current binary to `/usr/local/bin/kwork-parser.previous` when one
   exists.
3. Installs the new artifact to a temporary path on the same filesystem and
   renames it atomically over `/usr/local/bin/kwork-parser`.
4. Restarts `kwork-parser`.
5. Polls systemd for a fixed health window. Success requires the service to stay
   active and its restart counter not to increase.
6. On failure, atomically restores `.previous`, restarts the service, verifies
   the restored process, and exits nonzero.
7. Removes the staging directory on either outcome.

The script uses strict shell settings, quotes all paths, applies restrictive
permissions, and never reads or logs application credentials.

## VPS Access Controls

- `kwork-deploy` has no password and no general-purpose sudo access.
- SSH authentication is key-only for this account.
- `authorized_keys` restricts the key where compatible with the deployment
  command, and the sudoers entry permits only the root-owned deployment script.
- The deployment script and sudoers file are writable only by root.
- The existing service continues to run as the unprivileged `kwork-parser`
  account with its systemd sandbox and memory limits.

## Failure Handling and Rollback

- Build, test, checksum, upload, validation, restart, or health-check failures
  fail the GitHub deployment.
- Failures before binary replacement leave the running version unchanged.
- Failures after replacement restore the immediately preceding binary.
- A failed rollback is reported explicitly and leaves systemd logs available for
  diagnosis; credentials are not included in diagnostics.
- Application state is not rolled back because its bounded JSON schema is
  backward-compatible within this release process. Any future incompatible
  state migration requires a separate design.

## Verification

Repository checks will validate workflow syntax and run shell linting where the
tooling is available. Deployment-script tests will exercise at least checksum
rejection, invalid staging paths, and rollback behavior using isolated fixtures
or command stubs.

After installation, one real push to `main` will exercise the complete pipeline.
Success requires:

- all CI jobs pass;
- GitHub records a successful `production` deployment;
- the VPS binary checksum matches the Actions artifact;
- `kwork-parser` remains active with no added restarts;
- Kwork polling remains operational;
- memory and task counts remain within the systemd limits.

## Documentation

The README will document required GitHub environment values, initial VPS
bootstrap, automatic deployment behavior, rollback behavior, key rotation, and
manual service inspection. It will continue to describe manual deployment as an
emergency fallback.

## Out of Scope

- Containerization or a container registry
- A self-hosted GitHub runner
- Multiple environments or canary rollout
- Automatic modification of Telegram or Kwork credentials
- Database or state migrations
