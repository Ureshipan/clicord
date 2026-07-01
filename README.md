# clicord

A terminal messenger in Rust — direct messages, group chats and (later) P2P
voice calls — with a server you can self-host on
[samoswallow](https://github.com/Ureshipan/samoswallow).

> Status: **early MVP.** Working today: registration/login, realtime 1:1
> direct messages over websockets, group chats, presence, file/image
> attachments (with inline terminal image previews), persistent unread badges,
> and a TUI client. Bots and calls are on the roadmap.

## Workspace layout

```
crates/
  protocol/   # shared wire types (serde) — the single source of truth for the contract
  server/     # axum: /health, /api/{register,login,upload}, /api/attachment/:id, /ws + SQLite + in-memory routing hub
  client/     # ratatui + crossterm TUI client
Dockerfile    # builds the server image
swallow.yaml  # samoswallow deployment descriptor
```

The three layers in the client (`net` / `app` / `ui`) are deliberately
separate so the interface can be reworked without touching networking, and
vice-versa.

## Run locally

Terminal 1 — the server:

```sh
CLICORD_JWT_SECRET=dev-secret cargo run -p server
# listens on http://0.0.0.0:8080
```

Terminal 2 (and 3, 4, …) — one client per user:

```sh
cargo run -p client                  # on first run it asks for the server address
cargo run -p client -- http://host   # optional: prefill the address on first run
```

The server address is asked once on first run and saved to
`~/.config/clicord/config.json`; the CLI argument only prefills it. Change it
later from the connection-error screen.

In the client:

- **Server setup** (first run): type the address, `Enter` to save.
- **Accounts screen:** `↑/↓` to pick, `Enter` to connect with the saved token,
  `a` to add another, `d` to delete, `q` to quit. You can also click a row.
- **Login screen:** `user`/`pass` fields — `Tab` switches field, `Ctrl+R`
  toggles login/register, `Enter` submits. Logins are saved to the account
  store (`~/.config/clicord/sessions.json`).
- **Chat screen:** `/dm <user>` opens a direct conversation, `/find <prefix>`
  searches registered users, `/group <name> [users...]` creates a group and
  `/g <name>` opens one (or **click a name** in the list — DMs and groups live
  there together). Type and `Enter` to send. `Tab` autocompletes commands,
  usernames and group names. Edit with `←/→`, `Home/End`, `Delete`. Unread chats
  show a red badge — including messages that arrived while you were **offline**
  (the read position is tracked server-side, so badges survive restarts and stay
  in sync across your devices). `Esc` or `/accounts` returns to the session
  manager.
- **Attachments:** `/file <path>` (alias `/attach`, `/f`) uploads a file or
  image to the open chat — the rest of the line is the path, so spaces are fine.
  Attachments show under the message with an index, name and size. Images get an
  inline preview drawn with Unicode half-blocks (works on any truecolour
  terminal — Windows, macOS, Linux — no sixel/kitty required). `/view <n>`
  downloads attachment *n* and opens it in the OS's default application.
- **Message history** groups messages by day: a `[DD.MM.YY]` divider separates
  days, and each message still shows its send time.
- **Scrollback:** `PgUp/PgDn`, `↑/↓` or the mouse wheel scroll the history. At
  the bottom it sticks to the newest message; scroll up and incoming messages
  no longer shift the view — a `▼ N new` marker appears instead, and the chat's
  unread badge counts up until you return to the bottom.
- **Typing indicators:** while someone types in the open conversation, the input
  box title shows `… is typing…` (it clears after a few seconds of silence).
- **Connection lost / unreachable:** a screen offers `r` retry, `s` change
  server, `a` accounts, `q` quit.
- **`F1`** shows all commands and keybindings; **`Ctrl+Q`** quits from anywhere.

The same account can be open in several terminals at once — messages stay in
sync across all of them. Open two clients, register two users, `/dm` each other
and chat.

## Configuration (env vars)

| Variable             | Default                       | Purpose                          |
|----------------------|-------------------------------|----------------------------------|
| `CLICORD_LISTEN`     | `0.0.0.0:8080`                | bind address                     |
| `CLICORD_DB`         | `sqlite://clicord.db`         | SQLite connection string         |
| `CLICORD_JWT_SECRET` | *(auto-generated)*            | secret for signing session tokens |

`CLICORD_LISTEN`/`CLICORD_DB` map onto the `env:` block in `swallow.yaml`.

**The JWT secret is never committed.** If `CLICORD_JWT_SECRET` is unset, the
server generates a random secret on first boot and persists it in the database
(i.e. under `/data` on a mounted volume), so it survives restarts but lives
only on the private volume. Set `CLICORD_JWT_SECRET` explicitly (e.g. as a
samoswallow encrypted Secret, once available) to override it.

## Deploy on samoswallow

`swallow.yaml` + `Dockerfile` are ready: point samoswallow at this repo, set a
real `CLICORD_JWT_SECRET` in the UI, and it builds the image, runs the
container and routes `clicord.<base-domain>` to it. Health is polled at
`/health`. Keep `default_instances: 1` until the routing hub is backed by a
shared bus (see roadmap).

## Releases (cross-platform client binaries)

Push a version tag and GitHub Actions builds the client for Linux
(gnu + musl), Windows and macOS (x86_64 + arm64) and attaches them to a
GitHub Release:

```sh
git tag v0.1.0
git push origin v0.1.0
```

See `.github/workflows/release.yml`. The server isn't shipped as a binary — it
runs as a container. To build the client locally for a target, use
`scripts/build-release.sh [target...]` (host target with cargo; others via
[`cross`](https://github.com/cross-rs/cross)).

## Roadmap

1. ✅ Auth + 1:1 DMs + presence
2. ✅ Group chats + user search
3. ✅ Scrollback (stick-to-bottom) + typing indicators
4. Read receipts; message history pagination
5. Bot tokens (bots are just clients with a bot credential)
6. Voice calls: server-side signaling (SDP/ICE over `/ws`) + P2P media via
   WebRTC, with a TURN relay fallback
