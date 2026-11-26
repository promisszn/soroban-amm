const test = require('node:test')
const assert = require('node:assert/strict')
const {
  Account,
  Contract,
  Keypair,
  Networks,
  SorobanDataBuilder,
  StrKey,
  Transaction,
  TransactionBuilder,
  xdr,
} = require('@stellar/stellar-sdk')
const { SorobanRpcAdapter: Adapter, SorobanSubmitter: Submitter } = require('../lib/index.js')

const source = Keypair.random()
const contract = new Contract(StrKey.encodeContract(Buffer.alloc(32, 1)))

function makeTransaction(sequence = '1') {
  return new TransactionBuilder(
    new Account(source.publicKey(), sequence),
    { fee: '100', networkPassphrase: Networks.TESTNET },
  )
    .addOperation(contract.call('invoke'))
    .setTimeout(30)
    .build()
}

function makeSimulation({ restore = false } = {}) {
  const data = new SorobanDataBuilder()
    .setResources(100_000, 1_000, 1_000)
    .setResourceFee('123')
  const transactionData = new SorobanDataBuilder(data.build())
  const response = {
    id: '1',
    latestLedger: 1,
    events: [],
    _parsed: true,
    transactionData,
    minResourceFee: '123',
    result: { auth: [], retval: xdr.ScVal.scvVoid() },
  }
  if (restore) {
    response.restorePreamble = {
      minResourceFee: '456',
      transactionData: new SorobanDataBuilder(data.build()),
    }
  }
  return response
}

function makeServer(overrides = {}) {
  return {
    async simulateTransaction() {
      return makeSimulation()
    },
    async getAccount(address) {
      return new Account(address, '2')
    },
    async sendTransaction() {
      return { status: 'PENDING', hash: 'HASH' }
    },
    async getTransaction() {
      return { status: 'SUCCESS', returnValue: xdr.ScVal.scvVoid() }
    },
    ...overrides,
  }
}

test('SorobanRpcAdapter maps Stellar RPC simulation, assembly, send, and result', async () => {
  const adapter = new Adapter({ server: makeServer() })
  const tx = makeTransaction()
  const simulation = await adapter.simulateTransaction(tx)

  assert.equal(simulation.minResourceFee, 123n)
  assert.equal(simulation.restoreRequired, false)
  assert.ok(await adapter.assembleTransaction(tx, simulation) instanceof Transaction)
  assert.deepEqual(await adapter.sendTransaction(tx), { status: 'PENDING', hash: 'HASH' })
  assert.deepEqual(await adapter.getTransaction('HASH'), {
    status: 'SUCCESS',
    returnValue: xdr.ScVal.scvVoid(),
  })
})

test('SorobanSubmitter restores expired footprint before simulating the invocation again', async () => {
  const calls = []
  let simulationCount = 0
  let sendCount = 0
  const submittedSequences = []
  const adapter = new Adapter({
    server: makeServer({
      async simulateTransaction() {
        calls.push('simulate')
        simulationCount += 1
        return makeSimulation({ restore: simulationCount === 1 })
      },
      async getAccount(address) {
        calls.push('getAccount')
        return new Account(address, String(1 + calls.filter((call) => call === 'getAccount').length))
      },
      async sendTransaction(tx) {
        calls.push(tx.operations[0].type === 'restoreFootprint' ? 'sendRestore' : 'sendInvocation')
        submittedSequences.push(tx.sequence)
        sendCount += 1
        return {
          status: 'PENDING',
          hash: sendCount === 1 ? 'RESTORE_HASH' : 'INVOCATION_HASH',
        }
      },
      async getTransaction(hash) {
        calls.push(`poll:${hash}`)
        return { status: 'SUCCESS', returnValue: xdr.ScVal.scvVoid() }
      },
    }),
  })
  const submitter = new Submitter({
    rpc: adapter,
    signer: {
      async sign(tx) {
        calls.push(tx.operations[0].type === 'restoreFootprint' ? 'signRestore' : 'signInvocation')
        return tx
      },
    },
    sleep: async () => {},
  })

  await submitter.sendTransaction(makeTransaction())

  assert.deepEqual(calls, [
    'simulate',
    'getAccount',
    'signRestore',
    'sendRestore',
    'poll:RESTORE_HASH',
    'getAccount',
    'simulate',
    'signInvocation',
    'sendInvocation',
    'poll:INVOCATION_HASH',
  ])
  assert.deepEqual(submittedSequences, ['3', '4'])
})

test('SorobanSubmitter completes simulate, assemble, sign, send, and poll through a fake server', async () => {
  const calls = []
  const adapter = new Adapter({
    server: makeServer({
      async simulateTransaction() {
        calls.push('simulate')
        return makeSimulation()
      },
      async sendTransaction() {
        calls.push('send')
        return { status: 'PENDING', hash: 'HASH' }
      },
      async getTransaction() {
        calls.push('poll')
        return { status: 'SUCCESS', returnValue: xdr.ScVal.scvVoid() }
      },
    }),
  })
  const assemble = adapter.assembleTransaction.bind(adapter)
  adapter.assembleTransaction = async (...args) => {
    calls.push('assemble')
    return assemble(...args)
  }
  const submitter = new Submitter({
    rpc: adapter,
    signer: {
      async sign(tx) {
        calls.push('sign')
        return tx
      },
    },
  })

  await submitter.sendTransaction(makeTransaction())

  assert.deepEqual(calls, ['simulate', 'assemble', 'sign', 'send', 'poll'])
})
