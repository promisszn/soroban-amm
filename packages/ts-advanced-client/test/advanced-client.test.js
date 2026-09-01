const test = require('node:test')
const assert = require('node:assert/strict')
const {
  AdvancedClient,
  MiddlewareStack,
  priceFeedMiddleware,
  signerMiddleware,
} = require('../lib/index.js')

test('runs middleware in order and submits the processed transaction', async () => {
  const calls = []
  const submitter = {
    async sendTransaction(tx) {
      calls.push(['submit', tx])
      return { hash: 'abc123', tx }
    },
  }
  const client = new AdvancedClient(submitter)
  client
    .middleware()
    .use(priceFeedMiddleware(async () => 123.45))
    .use(signerMiddleware({ sign: async (tx) => ({ ...tx, signed: true }) }))
    .use(async (ctx, next) => {
      calls.push(['middleware', ctx.metadata.price])
      await next()
    })

  const result = await client.sendTransaction({ kind: 'swap', amount: 1000 })

  assert.deepEqual(calls, [
    ['middleware', 123.45],
    ['submit', { kind: 'swap', amount: 1000, signed: true }],
  ])
  assert.deepEqual(result, {
    hash: 'abc123',
    tx: { kind: 'swap', amount: 1000, signed: true },
  })
})

test('does not submit when middleware fails', async () => {
  let submitted = false
  const client = new AdvancedClient({
    async sendTransaction() {
      submitted = true
      return { ok: true }
    },
  })
  client.middleware().use(async () => {
    throw new Error('validation failed')
  })

  await assert.rejects(() => client.sendTransaction({ kind: 'swap' }), /validation failed/)
  assert.equal(submitted, false)
})

test('rejects calling next more than once', async () => {
  const stack = new MiddlewareStack()
  stack.use(async (_ctx, next) => {
    await next()
    await assert.rejects(() => next(), /next\(\) called multiple times/)
  })

  await stack.run({ tx: { kind: 'swap' }, metadata: {} })
})
