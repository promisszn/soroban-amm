# Soroban AMM health dashboard

The dashboard is a dependency-free browser client for `services/graphql-api`. It reads `poolStats`, `poolHealth`, `alertConfigs`, `priceHistory`, and recent pool events through GraphQL, then polls for updates without requiring a rebuild.

## Run

Start the API on its default port and serve this directory:

```sh
npm install
npm run dev
```

Open `http://localhost:3000/?pool=<POOL_ID>`. The endpoint can be changed without editing source by using `?api=http://localhost:4000/graphql` (or by editing the visible GraphQL API field). `?interval=10000` changes polling to ten seconds. The default endpoint is `http://localhost:4000` and the default pool is empty, which produces the explicit “no pools indexed yet” state.

The connection indicator reports connecting, connected, degraded, or error. Each refresh clears values before requesting data, so a failed request cannot leave stale numbers presented as current. Partial query failures retain successful sections and identify failed sections; a total failure shows a readable message, hover details, and a retry button. Alert configuration uses `setAlertConfig` and `removeAlertConfig` mutations with optimistic updates and rollback on failure.

The buildless package provides `npm run build`, `npm run lint`, and `npm test`. The dashboard logic and transformation helpers live in `src/dashboard.js`, with smoke coverage in `src/dashboard.test.js`.
