# isro-notifications

Watches the ISRO homepage and sends new flash and latest news to Telegram.

## How it works

1. **Fetch** - polls the page every `poll_interval_secs`, keeps the last snapshot in `.state/last.html`, and reports when the checksum changes.
2. **Diff** - parses the flash news carousel and the latest news accordion and reports additions only. Removals are ignored. Blocked or maintenance pages are reported as failures.
3. **Enrich** - for internal links, fetches the article, extracts images and videos, and asks Sarvam for a short summary that keeps the original title. External links pass through as title + link.
4. **Deliver** - sends to every registered Telegram chat. Failures go to the admin.

## Requirements

- Rust (edition 2024)
- A Telegram bot token
- Optional: a Sarvam API key. Without it, updates are sent as title + link.

## Setup

```bash
git clone <repo>
cd isro-notifications
cp .env.example .env
```

`.env` holds secrets only:

```dotenv
SARVAM_API_KEY=sk_...
TELEGRAM_BOT_TOKEN=123456:AA...
# ANALYTICS_BIND=127.0.0.1:8080
```

`config.yaml` holds everything else:

```yaml
isro:
  url: https://www.isro.gov.in
  base_url: https://www.isro.gov.in
  poll_interval_secs: 1800
  state: ./.state/last.html

summary:
  max_chars: 1024
  concurrency: 4

telegram:
  registry: ./.state/telegram.json
  # admin_username: your_username
  welcome: "👋 You're subscribed. ISRO updates will arrive here."
  concurrency: 2
  alerts:
    fetch_failed: "fetch failed: {error}"
    site_failure: "site failure: {message}"
```

| Field | Meaning |
|---|---|
| `isro.url` | page to poll |
| `isro.base_url` | canonical site, used to resolve links and decide internal vs external |
| `isro.poll_interval_secs` | poll interval |
| `isro.state` | last snapshot, so a restart diffs instead of re-baselining |
| `summary.max_chars` | max length of the whole message, link included |
| `summary.concurrency` | articles fetched and summarised at once |
| `telegram.registry` | chat registry |
| `telegram.admin_username` | receives failure alerts; must have started the bot |
| `telegram.welcome` | reply to `/start` |
| `telegram.concurrency` | chats sent to at once |
| `telegram.alerts.*` | alert templates; `{error}` and `{message}` are substituted |

## Telegram

1. Create a bot with [@BotFather](https://t.me/BotFather) and copy the token into `.env`.
2. Send `/start` to the bot. The service polls `getUpdates` every 60s and adds the chat to the registry.
3. Set `telegram.admin_username` to your username to receive failure alerts.

## Run

```bash
cargo run --release
```

Logs go to stderr. Use `RUST_LOG=debug` for more.

## Analytics

`http://127.0.0.1:8080` by default. Set `ANALYTICS_BIND` to change the address (IP or hostname, e.g. a Tailscale MagicDNS name), or to an empty value to disable it.

## Smoke test

```bash
cargo run --bin smoke
```

Serves real snapshots from `.dev/` on a local port, runs 6 checks 30 seconds apart, and uses the real Sarvam and Telegram. Needs a registered chat. Takes about 2.5 minutes.

## Tests

```bash
cargo test
```

The real Sarvam test is ignored by default:

```bash
cargo test --test enrich -- --ignored
```

## Dev fetch

```bash
cargo run --bin fetch -- https://www.isro.gov.in
```

Saves the page to `$CACHE_DIR` (default `./`) with a timestamped name.

## systemd

Copy `deploy/isro-notifications.service` to `/etc/systemd/system/`, edit `WorkingDirectory`, `ExecStart` and `User`, then:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now isro-notifications
journalctl -u isro-notifications -f
```

`WorkingDirectory` matters because `.env`, `config.yaml` and `.state/` are relative.

## Notes

- The last snapshot is persisted, so a restart reports what changed while it was down. The first ever start baselines and sends nothing.
- Run one instance only. Two would double-send and race on the registry offset.
- `.env` and `.state/` are gitignored.
