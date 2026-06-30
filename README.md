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
cargo run -p client            # connects to http://127.0.0.1:8080
cargo run -p client -- http://my-host:8080   # or a custom server
```

In the client:

- **Login screen:** type a username/password, `Tab` switches field, `Ctrl+R`
  toggles login/register, `Enter` submits.
- **Chat screen:** `/dm <user>` opens a conversation, then just type and press
  `Enter`. `/help` lists commands, `/quit` (or `Esc`) exits.

Open two clients, register two users, `/dm` each other and chat.

## Configuration (env vars)

| Variable             | Default                       | Purpose                          |
|----------------------|-------------------------------|----------------------------------|
| `CLICORD_LISTEN`     | `0.0.0.0:8080`                | bind address                     |
| `CLICORD_DB`         | `sqlite://clicord.db`         | SQLite connection string         |
| `CLICORD_JWT_SECRET` | *(insecure dev default)*      | secret for signing session tokens |

These map directly onto the `env:` block in `swallow.yaml`.

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
