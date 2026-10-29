# Takosumi Mobile push relay

The push relay lets apps opened inside Takosumi Mobile use standard Web Push.
To an app it is an ordinary RFC 8030 push service, like FCM's or Apple's: the
app sends an RFC 8291 `aes128gcm` message with RFC 8292 VAPID to the endpoint
its page received from `pushManager.subscribe()`. The relay forwards the still
encrypted body to the device through APNs or FCM, and Takosumi Mobile decrypts
and shows it on the device.

The relay is part of Takosumi Mobile and is run by its publisher. It is not part
of Takosumi: Takosumi's control plane, API and dashboard never see a push.
Design: takos-control `docs/platform/takosumi-mobile-browser.md` (「push中継」)
and ADR 0026.

It is a Cloudflare Worker with a D1 database. It needs no dependency beyond
what the repository already has; `relay/tsconfig.json` checks it, and
`bun run relay:check` runs its typecheck and tests.

## Device API (Takosumi Mobile only)

| Method | Path | Body | Answer |
| --- | --- | --- | --- |
| POST | `/v1/devices` | `{platform: "apns" \| "fcm", token, apns_environment?}` | 201 `{device_id, device_secret}` |
| PUT | `/v1/devices/{device_id}` | `{token, apns_environment?}` | 204 |
| DELETE | `/v1/devices/{device_id}` | | 204; the device and all its subscriptions are gone |
| POST | `/v1/devices/{device_id}/subscriptions` | `{application_server_key}` | 201 `{subscription_id, endpoint}` |
| DELETE | `/v1/devices/{device_id}/subscriptions/{subscription_id}` | | 204, or 404 |

- Everything after registration needs `Authorization: Bearer <device_secret>`.
  An unknown device and a wrong secret both answer 401; the client registers
  again.
- `apns_environment` is `production` (default) or `development` (sandbox).
- `application_server_key` is the page's `applicationServerKey`: an uncompressed
  P-256 public key, base64url. The relay rejects anything that is not a point
  on the curve.
- `endpoint` is `https://<relay>/push/<endpoint_id>` with 256 random bits.
  Replacing the token never changes an endpoint, so apps never re-register.
- A device whose token APNs/FCM rejected, or that was superseded, cannot add
  subscriptions (409) until it sends a new token.
- **One active device per token.** Registering (or moving a device to) a token
  that another active device holds supersedes that device: it is marked invalid,
  its endpoints answer 410, and the apps behind them subscribe again. A partial
  unique index enforces it (`migrations/0002_one_active_device_per_token.sql`).
  Accepted trade-off: someone who knows a device's push token can reset that
  device's subscriptions; push tokens are not public.
- **Activity.** Every authenticated call (token update, subscription changes)
  refreshes the device's `seen_at`. Devices unseen for 180 days are deleted by
  the daily cron, and their endpoints answer 404. Takosumi Mobile should `PUT`
  its token on launch and whenever it changes.
- Token formats: APNs exactly 64 hex characters; FCM at most 1,024 characters
  of `[A-Za-z0-9_:-]`.
- Limits: 30 registrations per hour per client (an IPv4 address, or an IPv6
  /64, since one subscriber usually holds a whole /64), 3,000 registrations per
  hour over all clients, 120 new subscriptions per device per hour, 64
  subscriptions per device, 8 device rows (active or superseded) per token.

## Push resource (application servers)

`POST /push/{endpoint_id}`

| Check | Answer |
| --- | --- |
| Unknown endpoint | 404 |
| No `Authorization: vapid t=…, k=…`, or not parseable | 401 with `WWW-Authenticate: vapid` |
| `k` is not the subscription's application server key; bad ES256 signature; `aud` is not the relay origin; `exp` past or more than 24 hours away | 403 |
| APNs/FCM has reported the device invalid | 410 |
| `Content-Encoding` is not `aes128gcm` | 415 |
| No `TTL`, or not whole seconds; bad `Urgency`; `Topic` not ≤ 32 base64url characters | 400 |
| Body above 2,800 bytes (declared or streamed) | 413 |
| Body is not one aes128gcm record with a 65-byte key id | 400 |
| More than 60 messages per endpoint per minute, or past the device's ceiling and fair share (below) | 429 with `Retry-After` |
| No sender configured for the device's platform | 503 |
| APNs/FCM throttled / unavailable | 429 / 503 |
| APNs/FCM refused the relay's own configuration, or the invalidation breaker is open | 502 (the device is not touched) |
| The device replaced its token while the push was in flight | 503 with `Retry-After: 5` |
| Accepted | 201 with `Location` and the applied `TTL` |

- Only requests that pass VAPID count against the limits.
- **Fair share per device.** A device may receive 240 messages a minute over
  all its endpoints. Past that ceiling an endpoint may still send while it is
  within its fair share: 240 divided by the device's endpoints that sent this
  minute. Busy apps can use the device's budget, but an app that sends little
  always gets its share. Refused messages do not count against the device.
- `TTL` above 28 days is reduced and the response says so in `TTL`.
- 2,800 bytes is what fits APNs's and FCM's 4,096-byte payload once base64url
  encoded with the envelope; the sender tests check both at exactly this size.
  Apps should keep plaintext at or below about 2,048 bytes.
- Messages are not retained: `Location` names `/message/{id}`, which answers 404.

## Delivery

Both platforms carry only `{s: subscription_id, b: base64url(body)}`.

- **APNs**: `apns-push-type: alert`, an alert with placeholder text
  (`Takosumi` / `新しい通知`) and `mutable-content: 1`, so the Notification
  Service Extension decrypts and replaces it. `apns-priority` 10 for `high` and
  `normal`, 5 below; `apns-expiration` from `TTL` (0 for `TTL: 0`);
  `apns-collapse-id` from `Topic`; `apns-topic` is the app's bundle id. Token
  authentication with the `.p8` key; the provider JWT's `iat` comes from a
  shared 40-minute bucket, so concurrent isolates present the same issue time
  and no token is older than 40 minutes.
- **FCM**: HTTP v1 data message; `android.priority` HIGH for `Urgency: high`,
  NORMAL otherwise; `android.ttl` from `TTL`; `android.collapse_key` from
  `Topic`. OAuth 2.0 with the service account key; the access token is cached
  per isolate.
### Which answers invalidate a device

Only answers about the one token may invalidate a device: APNs
`BadDeviceToken`, `Unregistered`, `ExpiredToken` (410), and FCM `UNREGISTERED`
or an invalid registration token. Pushes then answer 410 until the device sends
a new token; after 30 days the daily cron deletes the device and its endpoints
answer 404.

Answers about the relay's own configuration are relay failures, answered 502,
and never touch the device: APNs `DeviceTokenNotForTopic` (a wrong
`APNS_TOPIC` gets this for every device), `TopicDisallowed`, `MissingTopic`,
`BadTopic`, `InvalidProviderToken`, `ExpiredProviderToken`,
`MissingProviderToken`, `BadEnvironmentKeyInToken`, `Forbidden`, certificate
errors, and FCM `SENDER_ID_MISMATCH` (a wrong Firebase project gets this for
every device) and any 401/403. Unknown answers are failures too. Apps retry 5xx
and keep their subscriptions.

Two more guards keep a mistake from erasing subscriptions:

- **Invalidation breaker.** Per platform, in a 10-minute window, if
  invalidations exceed 20, or exceed a quarter of deliveries once there were
  20 deliveries, the relay stops invalidating, answers 502 and logs
  `invalidation_breaker_open` at error level with `alert: true` and the counts.
  It closes when the window passes. Tune the thresholds in `DEFAULT_LIMITS`
  (`src/context.ts`) as traffic grows.
- **Token check.** A device is invalidated only while it still holds the token
  that was rejected. If the device replaced its token while the push was in
  flight, nothing is invalidated and the push answers 503 to be retried.

The senders implement one interface (`PushSender` in `src/senders/types.ts`)
and take their `fetch` as a parameter; the tests drive them with mocks.

### APNs over HTTP/2 from a Worker (observed 2026-10-29)

APNs accepts only HTTP/2. A throwaway Worker deployed to workers.dev (KIX colo)
POSTed to `api.push.apple.com` and `api.sandbox.push.apple.com` with `fetch()`
and got APNs's own answers — `403 {"reason":"MissingProviderToken"}` without a
token and `403 {"reason":"InvalidProviderToken"}` with a bogus one, each with an
`apns-id` header — which APNs only produces over HTTP/2. A raw TLS socket from
`cloudflare:sockets` also received an HTTP/2 SETTINGS frame. So the APNs sender
runs in the Worker; no other runtime is needed.

Not yet observed: a delivery with a real `.p8` key and device token. Local
`wrangler dev` (workerd) speaks HTTP/1.1 and fails against APNs with "Network
connection lost", so APNs can only be exercised from a deployed Worker. Should
Cloudflare ever stop negotiating HTTP/2, `ApnsSender` runs unchanged in any
runtime with an HTTP/2 `fetch` (Bun, Node with undici) behind a small forwarder.

## What is stored and logged

D1 holds the device token, a SHA-256 hash of the device secret, the APNs
environment, an invalidation time, the last authenticated call, and per
subscription the application server key and the endpoint id (`migrations/`).
Counters are keyed by endpoint id, device id, platform, or a hash of the client
address (IPv4 address or IPv6 /64), and expire. The relay never stores an app's origin or a message body, and never
logs a body, a token, a device secret, a VAPID JWT or its `sub` (the claim is
never read). The tests capture every log line and check this.

## Configuration

| Name | Kind | Meaning |
| --- | --- | --- |
| `DB` | D1 binding | the relay database |
| `RELAY_ENVIRONMENT` | var | `staging` or `production` (required) |
| `RELAY_ORIGIN` | var | the public https origin; endpoints are issued under it and VAPID `aud` must equal it. Required in production; in staging, unset means the request's origin (workers.dev) |
| `DELIVERY_MODE` | var | `live` (default) or `sink` (accept and drop; staging only, refused elsewhere) |
| `APNS_TEAM_ID`, `APNS_KEY_ID`, `APNS_TOPIC` | var | Apple team, `.p8` key id, app bundle id (`com.takosumi`) |
| `APNS_PRIVATE_KEY` | secret | the `.p8` key (PKCS#8 PEM) |
| `FCM_PROJECT_ID` | var | the Firebase project id |
| `FCM_SERVICE_ACCOUNT_JSON` | secret | the service account key JSON |

A platform is enabled only when all its settings are present; otherwise pushes
to its devices answer 503. A misconfigured Worker answers 503 to everything.

## Deploy

The repository's one entrypoint is `bun run deploy`; `--contract` prints the
surfaces and how each obligation is met (`takos.deploy-contract@v3`).

| Surface | What it changes | Lineage |
| --- | --- | --- |
| `push-relay-staging` | the staging Worker (code plus its generated config) | integration: any worktree |
| `push-relay` | production Worker code only | clean `main` at or behind `origin/main` |
| `push-relay-staging-schema` | D1 migrations on staging | clean `main` |
| `push-relay-schema` | D1 migrations on production (after staging, with an export) | clean `main` |

```bash
PUSH_RELAY_STAGING_D1_DATABASE_ID=<id> bun run deploy -- push-relay-staging
PUSH_RELAY_D1_DATABASE_ID=<id> PUSH_RELAY_ORIGIN=https://<relay> bun run deploy -- push-relay --platform apns --platform fcm
bun run deploy -- push-relay-staging-schema --reviewed-by <name>
bun run deploy -- push-relay-schema --reviewed-by <name>
```

Every surface first reads `wrangler d1 list` and refuses unless the database
id is the database with the expected name (`takosumi-push-relay-staging` or
`takosumi-push-relay`); production also refuses the staging database's id.
Each Worker deploy runs `bun run relay:check` once, bundles `src/index.ts` with
Bun once, uploads that exact file with `--no-bundle`, reads the serving version
back, and runs `bun run relay:smoke` (health, device, subscription, VAPID 401
and 403, unknown endpoint 404, an encrypted Web Push answered 201 in sink mode
or 410 per platform in live mode, device deletion). In live mode a
configuration or credential error comes back 502 and fails the smoke (tested
against a relay with mock senders). A fake token cannot prove the APNs topic or
Firebase project binding itself — APNs answers `BadDeviceToken` before it
checks the topic — so the first push to a real device is the last check; a
wrong binding then answers 502 and leaves every device valid. Staging defaults to
`DELIVERY_MODE=sink` (`PUSH_RELAY_STAGING_DELIVERY_MODE=live` with the
`PUSH_RELAY_APNS_*` / `PUSH_RELAY_FCM_PROJECT_ID` identifiers switches it).

### Provisioning (separate from deploys)

Creating resources, domains and secrets is provisioning, outside the deploy
entrypoint:

1. `wrangler d1 create takosumi-push-relay-staging` (or `takosumi-push-relay`),
   and apply `relay/migrations` to the new empty database.
2. Production only: create the Worker with its custom domain (the relay origin),
   the vars above, and the cron trigger `17 3 * * *` (daily purge).
3. Secrets, as a credential operation:
   `wrangler secret put APNS_PRIVATE_KEY --name <worker>` and
   `wrangler secret put FCM_SERVICE_ACCOUNT_JSON --name <worker>`.

The staging Worker `takosumi-push-relay-staging` on workers.dev runs in sink
mode until APNs and Firebase credentials exist. It was deployed from 39ca885
(before migration 0002); after this branch is merged, run
`push-relay-staging-schema` and then `push-relay-staging` to bring it current.

## What the publisher must provide

- An APNs auth key (`.p8`, Apple Developer account) with its key id and team id.
- A Firebase project for Android and a service account key for FCM HTTP v1.
- The relay's domain, and the go-ahead for the production deploy.

## Not yet done

- Takosumi Mobile's relay client, the injected Push API, key storage and
  decryption on the device (design steps 4 and 5).
- The device must drop a push whose `s` it does not know (a superseded
  installation's subscription, for example); the iOS Notification Service
  Extension must show nothing app-specific for it.
- A delivery through real APNs/FCM credentials and a real device.
