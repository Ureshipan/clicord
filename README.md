# clicord

A terminal messenger in Rust — direct messages, group chats and (later) P2P
voice calls — with a server you can self-host on
[samoswallow](https://github.com/Ureshipan/samoswallow).

> Status: **early MVP.** Working today: registration/login, realtime 1:1
> direct messages over websockets, presence, and a TUI client. Group chats,
> bots and calls are on the roadmap.

## Workspace layout

```
crates/
  protocol/   # shared wire types (serde) — the single source of truth for the contract
  server/     # axum: /health, /api/register, /api/login, /ws  + SQLite + in-memory routing hub
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
- **Chat screen:** `/dm <user>` opens a conversation (or **click a name**), then
  type and `Enter`. `Tab` autocompletes commands and, after `/dm `, usernames.
  Edit with `←/→`, `Home/End`, `Delete`. Unread chats show a red badge. `Esc` or
  `/accounts` returns to the session manager.
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

## Roadmap

1. ✅ Auth + 1:1 DMs + presence
2. Group chats & message history pagination
3. Typing indicators / read receipts
4. Bot tokens (bots are just clients with a bot credential)
5. Voice calls: server-side signaling (SDP/ICE over `/ws`) + P2P media via
   WebRTC, with a TURN relay fallback
