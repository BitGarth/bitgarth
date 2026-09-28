# BitGarth on Ubuntu with Snap

The Snap packages are for amd64 Ubuntu systems with snapd. `bitgarth-web` runs
as a service on `http://127.0.0.1:8080`; `bitgarth-cli` is a separate command.
Install either package independently when it becomes available in the Snap
Store:

```sh
sudo snap install bitgarth-web
sudo snap install bitgarth-cli
bitgarth-cli --help
```

The web service starts automatically. Open `http://127.0.0.1:8080` on the
same machine. It listens only on loopback. Do not start a second
`bitgarth-web` process while the Snap service is running; both would try to
use the same port.

If another program already uses port 8080, choose a different port. The
service restarts on the new port; `snap unset` returns it to 8080:

```sh
sudo snap set bitgarth-web port=8081
sudo snap unset bitgarth-web port
```

The CLI can pair with this service, the hosted service at
`https://my.bitgarth.app`, or another BitGarth instance that you choose.
`login` works as another name for `pair`. The local service uses plain HTTP,
so pairing with it needs `--allow-insecure-http`:

```sh
bitgarth-cli pair http://127.0.0.1:8080/ --allow-insecure-http
bitgarth-cli balancesheet
```

The package provides `bitgarth-cli`. If you want the shorter `bitgarth`
command, enable a local alias:

```sh
sudo snap alias bitgarth-cli bitgarth
```

Check for a command conflict if another installation already provides
`bitgarth`.

## Service and logs

```sh
snap services bitgarth-web
sudo snap logs bitgarth-web -n=50
sudo snap restart bitgarth-web
sudo snap stop --disable bitgarth-web
sudo snap start --enable bitgarth-web
```

If the page does not load, check that the service is active, its port
(8080 unless changed with `snap set`) is free, and the machine's browser can
reach `127.0.0.1` on that port. The service's
logs can contain private details; redact them before sharing.

## Private data and updates

The web service keeps its private data under
`/var/snap/bitgarth-web/common/bitgarth`: wallet data lives in encrypted
per-user databases, while login and app metadata live in an unencrypted app
database. The CLI keeps its profiles and
Client Keys under
`~/snap/bitgarth-cli/common/.config/BitGarth/cli/profiles.json` for each
Linux user. Keep access to these paths restricted. No existing Homebrew or
manual installation is imported automatically.

Snapd refreshes Store-installed packages automatically. The web data uses
`$SNAP_COMMON` and CLI profiles use `$SNAP_USER_COMMON`; these common
directories persist across revisions. Reverting the Snap executable does
not undo a database migration or other changes to common data.

Before an intentional downgrade or a risky migration, stop the web service
and create a named snapshot. Note the `Set` ID printed by `snap save`:

```sh
sudo snap stop bitgarth-web
sudo snap save bitgarth-web bitgarth-cli
sudo snap check-snapshot SET_ID
sudo snap start bitgarth-web
```

Replace `SET_ID` with the printed number. The snapshot contains private
account and profile data, so protect any exported copy. Restore a snapshot
only as a deliberate recovery operation: restoring replaces newer data.
Do not use `snap remove --purge` for routine troubleshooting; it discards
data without a removal snapshot. See Snap's [snapshot guide](https://snapcraft.io/docs/how-to-guides/manage-snaps/create-data-snapshots/)
and [data directory reference](https://snapcraft.io/docs/reference/development/environment-variables/).
