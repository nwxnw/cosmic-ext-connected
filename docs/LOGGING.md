# Logging and diagnostics

Connected emits structured tracing events directly to the user's systemd journal via the `tracing_journald` layer in `src/main.rs`.

**Live tail:**

```bash
journalctl --user SYSLOG_IDENTIFIER=cosmic-ext-connected -f
```

**Filter by level or message:**

```bash
journalctl --user SYSLOG_IDENTIFIER=cosmic-ext-connected -p warning      # WARN+
journalctl --user SYSLOG_IDENTIFIER=cosmic-ext-connected --grep "<text>"
journalctl --user SYSLOG_IDENTIFIER=cosmic-ext-connected _PID=<pid>      # one process at a time
```

The default filter directive depends on build profile: `error,cosmic_ext_connected=debug` for debug builds (`cargo build`) and `error,cosmic_ext_connected=warn` for release builds (`cargo build --release`, installed `.deb`/`.flatpak`). Every other crate logs errors only, so libcosmic and zbus failures still surface. Setting `RUST_LOG` overrides the default entirely:

```bash
RUST_LOG=cosmic_ext_connected=info               # one crate, raised
RUST_LOG=cosmic_ext_connected=trace,zbus=debug   # plus a dependency
```

The release default of `warn` keeps installed-build journald output to actionable problems only; raise it ad-hoc via `RUST_LOG` when debugging a deployed installation — but see the next section for how, because exporting it in a shell does **not** work for a panel-launched applet.

## Raising the level for an installed applet

**A panel-hosted applet does not inherit your shell environment.** cosmic-panel is spawned by `cosmic-session` at login, and it spawns the applet; your interactive shell is nowhere in that chain. Exporting `RUST_LOG` before running `journalctl` sets it for `journalctl`, which has no use for it — the applet keeps the compiled-in `warn` default and any `info`/`debug` events you were hoping for never exist. Verify with `tr '\0' '\n' < /proc/$(pgrep -f 'bin/cosmic-ext-connected' | head -1)/environ | grep RUST_LOG`.

**Running it outside the panel is not an option.** `cargo run` starts, but outside the panel the host container is the wrong size and the applet floods with resize messages, drowning anything you were trying to read. The applet must be hosted; raise the level in place instead. Two ways, cheapest first:

**Wrap the installed `Exec`.** Edit `~/.local/share/applications/io.github.nwxnw.cosmic-ext-connected.desktop`:
```
Exec=env RUST_LOG=cosmic_ext_connected=debug /home/<user>/.local/bin/cosmic-ext-connected
```

then `killall cosmic-panel`. Note `just install` rewrites that line, so re-apply after each install.

**Set it session-wide** for something persistent: add `export RUST_LOG=warn,cosmic_ext_connected=debug` to `~/.profile` and log out and back in. `start-cosmic` launches the session through a login shell, so the variable reaches cosmic-panel and the applet. Keep the leading `warn`: the variable applies to every process in the session, and a bare crate directive silences everyone else's warnings. Survives reinstalls; easy to forget to remove.

**Pick the level deliberately.** Some diagnostics that matter live at `debug`, not `info`, among them the range request (`requesting conversation range 0-<end> for thread <id>`, `sms/fetch.rs`), which Load older and the first-page re-read both send, and the `D-Bus signal: <iface>.<member>` line (`subscriptions.rs`), which is logged only for device-refresh signals, not for SMS, call or share signals. Running at `info` and seeing nothing is not evidence that nothing happened.

**The journal does not report bus order.** `D-Bus signal: <iface>.<member>` is logged in the *subscription*, when a signal is read off the stream (`subscriptions.rs`); app-side lines are logged in `update()`, when iced delivers the message. Thread-load signals have no stream-side line at all: the per-thread subscription forwards them silently, and every thread-load line is logged in `update()` by the store. The subscription can read and log the *next* signal before the previous one has been processed, so journal interleaving proves nothing about the order signals arrived on the bus. **`dbus-monitor` is the authority.** This has read as a contradiction of a measured finding before, which is the worst kind of trap - it invites "fixing" a correct design. Capture unfiltered, too: a `sender=` match rule on a well-known name is unreliable for signals (they carry unique sender names on the wire), and a wrong filter yields "sees nothing", indistinguishable from a confirmed hypothesis.

**Why direct routing:** cosmic-panel pipes applet stdout/stderr and re-emits each line through its own tracing tree, then drops INFO under its default `warn` filter. The journald layer bypasses this, preserving each event's original level under our own `SYSLOG_IDENTIFIER`. Inside Flatpak the layer may fail to construct (sandboxed journal socket) and silently falls back to fmt-only - and cosmic-panel then drops what it emits, so there is nothing to tail.

For a Flatpak build, log to a file instead. `~/.var/app/io.github.nwxnw.cosmic-ext-connected/` is writable from inside the sandbox and readable from the host, so add a `tracing_appender` (or plain `std::fs`) layer writing there, then `tail -f` it from the host. Remove it before shipping - it is a debugging aid, not a second logging backend.

**Adding diagnostics:** use `tracing::info!`/`warn!`/`error!` macros - they route through both layers automatically. For structured fields, prefer `tracing::info!(thread_id = %tid, "loaded")` over format-string interpolation; structured fields become separate journal fields (`thread_id` becomes `F_THREAD_ID`, filterable with `journalctl F_THREAD_ID=<id>`).
