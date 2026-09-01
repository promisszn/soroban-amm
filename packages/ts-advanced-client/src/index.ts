export type Transaction = object

export type MiddlewareContext<TTransaction extends Transaction = Transaction> = {
  tx: TTransaction
  metadata: Record<string, unknown>
}

export type Middleware<TTransaction extends Transaction = Transaction> = (
  ctx: MiddlewareContext<TTransaction>,
  next: () => Promise<void>,
) => Promise<void>

export interface TransactionSubmitter<
  TTransaction extends Transaction = Transaction,
  TResult = unknown,
> {
  sendTransaction(tx: TTransaction): Promise<TResult>
}

export class MiddlewareStack<TTransaction extends Transaction = Transaction> {
  private readonly middlewares: Array<Middleware<TTransaction>> = []

  use(middleware: Middleware<TTransaction>): this {
    this.middlewares.push(middleware)
    return this
  }

  async run(ctx: MiddlewareContext<TTransaction>): Promise<void> {
    let index = -1
    const dispatch = async (current: number): Promise<void> => {
      if (current <= index) throw new Error('next() called multiple times')
      index = current
      const middleware = this.middlewares[current]
      if (middleware) await middleware(ctx, () => dispatch(current + 1))
    }
    await dispatch(0)
  }
}

export class AdvancedClient<
  TTransaction extends Transaction = Transaction,
  TResult = unknown,
> {
  private readonly stack = new MiddlewareStack<TTransaction>()

  constructor(private readonly submitter: TransactionSubmitter<TTransaction, TResult>) {}

  middleware(): MiddlewareStack<TTransaction> {
    return this.stack
  }

  async sendTransaction(tx: TTransaction): Promise<TResult> {
    const ctx: MiddlewareContext<TTransaction> = { tx, metadata: {} }
    await this.stack.run(ctx)
    return this.submitter.sendTransaction(ctx.tx)
  }
}

export interface Signer<TTransaction extends Transaction = Transaction> {
  sign(tx: TTransaction): Promise<TTransaction>
}

export function signerMiddleware<TTransaction extends Transaction>(
  signer: Signer<TTransaction>,
): Middleware<TTransaction> {
  return async (ctx, next) => {
    ctx.tx = await signer.sign(ctx.tx)
    await next()
  }
}

export function priceFeedMiddleware<TTransaction extends Transaction = Transaction>(
  getPrice: () => Promise<number>,
): Middleware<TTransaction> {
  return async (ctx, next) => {
    ctx.metadata.price = await getPrice()
    await next()
  }
}
