# webhook-streamer

Push-based event streaming microservice for the Soroban AMM (issue #306).

Subscribes to Soroban contract events via Stellar RPC's `getEvents` method
and fans them out to registered HTTP webhooks.  Eliminates the need for
integrators (trading bots, analytics dashboards, notification services) to
poll Stellar RPC directly.

## Quick start

```bash
# Install dependencies
npm install

# Start in dev mode
CONTRACT_IDS=CABC123,CDEF456 npm run dev

# Build and start
npm run build && npm start
```

## Environment variables

| Variable          | Default                                    | Description                              |
|-------------------|--------------------------------------------|------------------------------------------|
| `SOROBAN_RPC_URL` | `https://soroban-testnet.stellar.org`      | Stellar RPC base URL                     |
| `HORIZON_URL`     | _(deprecated)_                             | Legacy alias for `SOROBAN_RPC_URL`; accepted for one release with a startup deprecation warning. `SOROBAN_RPC_URL` takes precedence. |
| `CONTRACT_IDS`    | _(empty)_                                  | Comma-separated contract IDs to watch    |
| `POLL_INTERVAL_MS`| `5000`                                     | Polling interval in milliseconds         |
| `PORT`            | `3001`                                     | Management API HTTP port                 |
| `WEBHOOK_ALLOW_PRIVATE_TARGETS` | `false`                       | Set to `true` to allow registering webhook URLs that point at loopback/link-local/private-range addresses. Only for local development against a same-host test receiver — leave unset in production, since it disables SSRF protection on `POST /webhooks`. |

## Management API

### Register a webhook
```
POST /webhooks
Content-Type: application/json

{
  "url": "https://your-server.com/hook",
  "contractId": "CABC123",   // optional — omit to receive all contracts
  "eventType": "swap",       // optional — omit to receive all event types
  "secret": "my-secret"      // optional — sent as X-Webhook-Secret header
}
```

`url` must be an `http`/`https` URL and must not target loopback, link-local
(including the cloud metadata address `169.254.169.254`), or RFC1918
private-range addresses — requests that fail this check return `400`. See
`WEBHOOK_ALLOW_PRIVATE_TARGETS` above to relax this for local development.

### List webhooks
```
GET /webhooks
```

### Unregister a webhook
```
DELETE /webhooks/:id
```

### Health check
```
GET /health
```

## Event payload

Each webhook receives a `POST` with a JSON body:

```json
{
  "id": "0000000012345678-0000000001",
  "contractId": "CABC123",
  "eventType": "swap",
  "ledger": 1234567,
  "timestamp": "2026-06-01T12:00:00Z",
  "schemaVersion": 1,
  "payload": {
    "zeroForOne": true,
    "amountIn": 1000000,
    "amountOut": 998000
  }
}
```

`schemaVersion` is the version carried by the contract's versioned event
envelope (`(EVENT_SCHEMA_VERSION, payload)`); subscribers can use it to tell
schema versions apart.  Topics and values are decoded from base64 XDR
`ScVal`s via `@stellar/stellar-sdk`.  `bigint` values are encoded as decimal
strings in the JSON delivered to webhooks.

Supported event types: `swap`, `add_liquidity`, `remove_liquidity`,
`mint_pos`, `burn_pos`, `coll_fees`, `mint_1t`, `rng_ord`, `staked`,
`unstaked`, `claimed`.  Unknown topics still pass through with their decoded
name.

## Delivery guarantees

- Up to 3 retries with exponential back-off (500 ms, 1 s, 2 s).
- Failed deliveries are logged but do not block other webhooks.
- Cursor-based pagination ensures no events are skipped between polls.
- A non-2xx response or an RPC `error` is logged at error level and surfaced
  on the health output; polling resumes on the next tick.
