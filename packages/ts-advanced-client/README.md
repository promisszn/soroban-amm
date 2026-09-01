# Advanced TypeScript Client with Middleware

This client provides a chainable middleware API for transaction signing, validation,
price feeds and plugins.

Usage example:

```ts
import {
  AdvancedClient,
  priceFeedMiddleware,
  signerMiddleware,
} from '@example/ts-advanced-client'

type Transaction = { kind: string; amount: number; signed?: boolean }
type Result = { hash: string }

const client = new AdvancedClient<Transaction, Result>({
  async sendTransaction(tx) {
    // Adapt this call to the Soroban SDK/RPC client used by your application.
    return sorobanRpc.sendTransaction(tx)
  },
})
client.middleware()
  .use(priceFeedMiddleware(async () => 123.45))
  .use(signerMiddleware({ sign: async (tx) => ({ ...tx, signed: true }) }))

const result = await client.sendTransaction({ kind: 'swap', amount: 1000 })
```
