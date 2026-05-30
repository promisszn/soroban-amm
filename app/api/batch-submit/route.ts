import { NextRequest, NextResponse } from "next/server";

// ── Types ─────────────────────────────────────────────────────────────────────

interface BatchSubmitBody {
  /** Client-generated UUID that uniquely identifies this submission attempt. */
  idempotencyKey: string;
  payments: PaymentItem[];
}

interface PaymentItem {
  destination: string;
  amount: string;
  asset: string;
  memo?: string;
}

interface IdempotencyRecord {
  jobId: string;
  createdAt: number; // Unix ms
}

// ── In-memory idempotency store ───────────────────────────────────────────────
//
// In production replace this map with a database-backed store (e.g. a `batch_idempotency_keys`
// table with a TTL index or a Redis SETEX). The 24-hour TTL ensures a client
// that retries after a network timeout receives the original response without
// creating a duplicate batch, while expiring stale keys to prevent unbounded
// growth.

const IDEMPOTENCY_TTL_MS = 24 * 60 * 60 * 1_000; // 24 hours

const idempotencyStore = new Map<string, IdempotencyRecord>();

function getIdempotencyRecord(key: string): IdempotencyRecord | null {
  const record = idempotencyStore.get(key);
  if (!record) return null;
  if (Date.now() - record.createdAt > IDEMPOTENCY_TTL_MS) {
    idempotencyStore.delete(key);
    return null;
  }
  return record;
}

function setIdempotencyRecord(key: string, jobId: string): void {
  idempotencyStore.set(key, { jobId, createdAt: Date.now() });
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function generateJobId(): string {
  return `job_${Date.now()}_${Math.random().toString(36).slice(2, 10)}`;
}

function isValidUUID(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
    value
  );
}

// ── Route handler ─────────────────────────────────────────────────────────────

export async function POST(req: NextRequest): Promise<NextResponse> {
  let body: BatchSubmitBody;

  try {
    body = (await req.json()) as BatchSubmitBody;
  } catch {
    return NextResponse.json(
      { error: "Invalid JSON body" },
      { status: 400 }
    );
  }

  // ── Idempotency key validation ─────────────────────────────────────────────

  if (!body.idempotencyKey) {
    return NextResponse.json(
      {
        error:
          "Missing required field: idempotencyKey. " +
          "Generate a UUID v4 on the client and include it with every submission. " +
          "Duplicate requests with the same key will return the original response " +
          "without reprocessing the batch.",
      },
      { status: 400 }
    );
  }

  if (!isValidUUID(body.idempotencyKey)) {
    return NextResponse.json(
      {
        error:
          "idempotencyKey must be a valid UUID v4 " +
          "(e.g. 550e8400-e29b-41d4-a716-446655440000).",
      },
      { status: 400 }
    );
  }

  // ── Duplicate detection ────────────────────────────────────────────────────

  const existing = getIdempotencyRecord(body.idempotencyKey);
  if (existing) {
    // Return the original job ID — no new batch is created.
    return NextResponse.json(
      {
        jobId: existing.jobId,
        duplicate: true,
        message:
          "This idempotency key was already used within the last 24 hours. " +
          "The original batch job ID is returned; no new batch was created.",
      },
      { status: 200 }
    );
  }

  // ── Payload validation ─────────────────────────────────────────────────────

  if (!Array.isArray(body.payments) || body.payments.length === 0) {
    return NextResponse.json(
      { error: "payments must be a non-empty array" },
      { status: 400 }
    );
  }

  // ── Process the new batch ──────────────────────────────────────────────────

  const jobId = generateJobId();

  // Persist the key before dispatching work so that a crash during processing
  // still prevents a second submission with the same key from creating a
  // duplicate batch.
  setIdempotencyRecord(body.idempotencyKey, jobId);

  // TODO: enqueue body.payments to the actual batch-processing worker.

  return NextResponse.json({ jobId, duplicate: false }, { status: 202 });
}
