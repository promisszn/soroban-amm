export interface ValidationError {
  row: number;
  column: string;
  message: string;
}

export interface ValidationReport {
  valid: boolean;
  errors: ValidationError[];
}

export interface PaymentRecord {
  destination: string;
  amount: string;
  asset: string;
  memo?: string;
}

const REQUIRED_PAYMENT_COLUMNS = ["destination", "amount", "asset"] as const;

/**
 * Parse raw CSV text into an array of row objects, collecting all validation
 * errors rather than throwing on the first failure.
 *
 * Each non-header row is validated for:
 *  - Missing required columns
 *  - Empty values in required columns
 *
 * @returns A tuple of [parsed rows (may be partial), ValidationReport].
 *          Callers must check `report.valid` before using the rows.
 */
export function parseCSV(
  csv: string,
  requiredColumns: string[] = []
): [Record<string, string>[], ValidationReport] {
  const errors: ValidationError[] = [];
  const rows: Record<string, string>[] = [];

  const lines = csv.split(/\r?\n/).filter((line) => line.trim().length > 0);
  if (lines.length === 0) {
    return [[], { valid: true, errors: [] }];
  }

  const headers = lines[0].split(",").map((h) => h.trim());

  // Validate that all required columns exist in the header.
  for (const col of requiredColumns) {
    if (!headers.includes(col)) {
      errors.push({
        row: 1,
        column: col,
        message: `Required column "${col}" is missing from the header row`,
      });
    }
  }

  for (let i = 1; i < lines.length; i++) {
    const rowNumber = i + 1; // 1-based, header is row 1
    const values = lines[i].split(",").map((v) => v.trim());
    const record: Record<string, string> = {};

    for (let j = 0; j < headers.length; j++) {
      record[headers[j]] = values[j] ?? "";
    }

    // Check required columns are non-empty in this row.
    for (const col of requiredColumns) {
      if (headers.includes(col) && !record[col]) {
        errors.push({
          row: rowNumber,
          column: col,
          message: `Required column "${col}" is empty`,
        });
      }
    }

    rows.push(record);
  }

  return [rows, { valid: errors.length === 0, errors }];
}

/**
 * Parse a payment CSV file, validating every row before allowing submission.
 *
 * Validates destination (non-empty Stellar address), amount (positive number),
 * and asset (non-empty string) across all rows simultaneously.
 *
 * @returns A tuple of [valid payment records, ValidationReport].
 *          The UI must display `report.errors` and block submission when
 *          `report.valid` is false.
 */
export function parsePaymentFile(
  csv: string
): [PaymentRecord[], ValidationReport] {
  const errors: ValidationError[] = [];

  const [rows, baseReport] = parseCSV(csv, [...REQUIRED_PAYMENT_COLUMNS]);

  // Accumulate structural errors from the base parse.
  errors.push(...baseReport.errors);

  const records: PaymentRecord[] = [];

  for (let i = 0; i < rows.length; i++) {
    const rowNumber = i + 2; // +1 for 0-index, +1 for header row
    const row = rows[i];

    const destination = row["destination"] ?? "";
    const amountStr = row["amount"] ?? "";
    const asset = row["asset"] ?? "";
    const memo = row["memo"];

    // Validate destination looks like a Stellar account ID.
    if (destination && !/^G[A-Z2-7]{55}$/.test(destination)) {
      errors.push({
        row: rowNumber,
        column: "destination",
        message: `"${destination}" is not a valid Stellar account ID (must start with G and be 56 characters)`,
      });
    }

    // Validate amount is a positive number.
    if (amountStr) {
      const amount = parseFloat(amountStr);
      if (isNaN(amount)) {
        errors.push({
          row: rowNumber,
          column: "amount",
          message: `"${amountStr}" is not a valid number`,
        });
      } else if (amount <= 0) {
        errors.push({
          row: rowNumber,
          column: "amount",
          message: `Amount must be greater than zero, got ${amountStr}`,
        });
      }
    }

    // Validate asset is non-empty (structural check already covers missing,
    // but a quoted empty string would pass the CSV split).
    if (asset !== undefined && asset.trim() === "" && amountStr !== "") {
      errors.push({
        row: rowNumber,
        column: "asset",
        message: "Asset code cannot be blank",
      });
    }

    records.push({ destination, amount: amountStr, asset, memo });
  }

  return [records, { valid: errors.length === 0, errors }];
}
