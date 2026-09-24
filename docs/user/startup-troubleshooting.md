# Startup troubleshooting

BitGarth checks its app database before starting the interface or background
tasks. If that check fails, it exits with status 1 and prints a diagnostic.
It also saves the diagnostic as `startup-error.txt` in the project directory
when it can write there. The dated file records the latest failed start and
can remain after a later successful start.

The project directory is the one selected by `BITGARTH_PROJECT_DIR`, or the
platform default when no override is set. The diagnostic prints the resolved
path. The Docker image normally uses `/data`, so the file is
`/data/startup-error.txt` inside its data volume.

If no file could be saved, run the executable from a terminal in its extracted
directory and read stderr. On Windows use `./bitgarth-web.exe` in PowerShell;
on macOS/Linux use `./bitgarth-web`. In Docker, inspect the container logs.

For incompatible migration history, stop every process using this project
directory and back up the whole directory, including user databases, envelopes,
`app/data/session-wrap-secret`, and SQLite sidecar files. If you supply
`BITGARTH_SESSION_WRAP_SECRET` externally, preserve its matching value securely
as part of your backup. Do not delete only `app.db`, remove WAL files, or edit
migration history. Waiting cannot repair incompatible history.

Use a build compatible with the complete database history. A migration name
does not reliably identify that build, and an older version is not always safe.
Test recovery using a copy of your backup.

To deliberately start an empty installation, select a new empty absolute
directory with `BITGARTH_PROJECT_DIR` and keep the original intact. This creates
a separate installation; it does not recover existing accounts or encrypted data.

For path, permissions, disk-space, or locking errors, correct the reported
cause before restarting. A successful `/health` response after startup reports
process liveness; it is not a continuous database readiness check.

The report stays in this instance and is not uploaded to BitGarth's servers.
It contains technical identifiers and filesystem paths, which may include your
OS username. Review and redact it before sharing it for support.

For a separate empty installation, choose a new, empty directory for each of
these examples. If `BitGarth-fresh` already contains data, choose another new
directory name. Run from the extracted package directory.

In PowerShell:

```powershell
$env:BITGARTH_PROJECT_DIR = Join-Path $env:LOCALAPPDATA 'BitGarth-fresh'
./bitgarth-web.exe
```

On macOS or Linux:

```sh
BITGARTH_PROJECT_DIR="$HOME/BitGarth-fresh" ./bitgarth-web
```

In Docker, select a new, empty data volume or bind mount at `/data` and retain
the original volume or directory intact. The new volume starts a separate
installation and does not recover accounts or encrypted data from the old one.
