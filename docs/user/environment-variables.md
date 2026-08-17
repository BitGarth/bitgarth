# Environment Variables

These settings configure a self-hosted BitGarth Docker container. Set them with
Docker Compose's `environment` section or `docker run --env`, then restart the
container for changes to take effect.

## Quick Reference

| Variable | Default | Required | Purpose |
|---|---|---|---|
| `IP` | `0.0.0.0` | No | Server bind address. |
| `PORT` | `8080` | No | Server bind port. |
| `BITGARTH_PROJECT_DIR` | `/data` | No | Persistent data directory. |
| `BITGARTH_INSTANCE_NOTICE_INFO` | Unset | No | Operator-supplied markdown banner. |
| `BITGARTH_SESSION_IDLE_TIMEOUT_MINUTES` | `60` | No | Session inactivity timeout. |
| `BITGARTH_SESSION_ABSOLUTE_TIMEOUT_MINUTES` | `1440` | No | Maximum session lifetime. |
| `BITGARTH_SESSION_WRAP_SECRET` | Generated automatically | No | Secret used to wrap session keys. |
| `BITGARTH_COOKIE_SECURE_POLICY` | `auto` | No | Controls the `Secure` flag on authentication cookies. |
| `BITGARTH_TRUST_PROXY_HEADERS` | `0` | No | Controls which proxy forwarding headers are trusted. |
| `RUST_LOG` | `info` | No | Log level and filter. |
| `RUST_BACKTRACE` | Unset | No | Includes backtraces in panic messages when set. |

## Server

### `IP`

The address on which the HTTP server listens. The Docker image defaults to
`0.0.0.0`, allowing Docker port publishing and other containers to reach it.
Control public access with Docker networking, published ports, a firewall, or a
reverse proxy.

### `PORT`

The HTTP server's listen port. The Docker image defaults to `8080`.

## Storage

### `BITGARTH_PROJECT_DIR`

The directory containing BitGarth's app database and per-user databases. The
Docker image uses `/data`; mount a persistent volume there so data survives
container replacement.

If you override this value, use an absolute path inside the container. BitGarth
creates the directory when it is missing; an existing path must be a writable
directory. The server refuses to start when the path is empty, relative, cannot
be created, or is not a directory.

The directory contains private financial data. Restrict access to the account
that runs BitGarth and include the volume in your backup plan.

Schema upgrades can change both the app database and encrypted per-user
databases. Before upgrading, stop BitGarth or take an application-consistent
snapshot of this entire resolved directory, including SQLite WAL files and
`app/data/session-wrap-secret`. A database-only copy is not a complete rollback
point.

## Instance Notice

### `BITGARTH_INSTANCE_NOTICE_INFO`

Markdown text displayed as an informational banner above the navigation on every
page. It is useful as notices from the operator.

Supported formatting after sanitization:

- Links using `http://`, `https://`, or `mailto:` URLs.
- Bold and italic text.
- Hard line breaks.

Headings, lists, code blocks, images, tables, and raw HTML are stripped or shown
as plain text. Unsafe links and tags are never executed.

The value is limited to 4096 bytes after surrounding whitespace is removed.
Unset, empty, oversized, or fully sanitized values do not display a banner.

Docker Compose example:

```yaml
services:
  bitgarth:
    environment:
      BITGARTH_INSTANCE_NOTICE_INFO: "Greeting all users. This is your operator speaking. Please have a wonderful day."
```

## Sessions And Reverse Proxies

### `BITGARTH_SESSION_IDLE_TIMEOUT_MINUTES`

The number of minutes without an authenticated request before a session is
invalidated. It must be a positive integer. Invalid values log a warning and use
the default of `60` minutes.

### `BITGARTH_SESSION_ABSOLUTE_TIMEOUT_MINUTES`

The maximum session lifetime in minutes from login, regardless of activity. It
must be a positive integer. Invalid values log a warning and use the default of
`1440` minutes (24 hours).

### `BITGARTH_SESSION_WRAP_SECRET`

An optional secret used to wrap session keys in the app database. When unset,
BitGarth generates a per-install secret and stores it under
`BITGARTH_PROJECT_DIR/app/data/session-wrap-secret`.

Set this only when you intend to manage the same secret across container
replacements. Treat it as sensitive and do not commit it to a Compose file.
When this variable is set externally, back up its value separately and restore
the matching value with any project-directory snapshot. A mismatched secret
invalidates existing sessions.

### `BITGARTH_COOKIE_SECURE_POLICY`

Controls the `Secure` flag on authentication cookies:

- `auto` (default): enable `Secure` for HTTPS requests.
- `always`: always enable `Secure`.
- `never`: never enable `Secure`.

Use `always` when a trusted reverse proxy terminates HTTPS and forwards plain
HTTP to the container.

### `BITGARTH_TRUST_PROXY_HEADERS`

Controls which forwarding headers the server trusts. It defaults to `0`
(disabled).

- `0`, `false`, `no`, or `off`: trust no forwarding headers.
- `proto`: trust `X-Forwarded-Proto` only. Client addresses still use the
  direct TCP peer.
- `1`, `true`, `yes`, or `on`: also trust `Forwarded` protocol metadata and
  `X-Forwarded-For` client addresses.

Enable either trust mode only when BitGarth is behind a trusted reverse proxy
that removes the corresponding client-supplied headers and writes its own
values.

Public pairing-start rate limits use the resolved source address. With proxy
trust disabled or set to `proto`, that address is the direct TCP peer. With full
proxy trust enabled, BitGarth requires exactly one valid `X-Forwarded-For`
address and rejects a missing, repeated, comma-separated, padded, or malformed
value. Before enabling pairing in a hosted deployment with full proxy trust,
prove that two real clients receive separate source buckets and that a
client-supplied spoofed header is removed or rejected.
Pairing state is in memory, so the deployment must also have exactly one live
BitGarth server process. If either invariant is unverified, block the public
pairing routes or do not deploy the pairing-enabled server release.

## Logging And Diagnostics

### `RUST_LOG`

Controls log filtering. Examples:

- `info`: informational messages and above.
- `debug`: debug messages and above.
- `bitgarth=debug,info`: debug messages for BitGarth and informational messages
  for other modules.

### `RUST_BACKTRACE`

Set to `1` or `full` to include a backtrace in panic messages. Backtraces may
contain implementation paths, so enable them only while diagnosing a problem.
