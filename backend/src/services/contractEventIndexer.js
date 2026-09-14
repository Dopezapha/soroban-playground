import config from '../config/index.js';
import DatabaseService from './databaseService.js';
import { parseEvent, dispatchEvent } from './contractEventParser.js';

const INSERT_EVENT_SQL = `
  INSERT INTO contract_events
    (contract_id, ledger_sequence, topics, value, raw_xdr, event_type)
  VALUES (?, ?, ?, ?, ?, ?)
`;

const LOAD_CURSOR_SQL =
  'SELECT last_ledger FROM contract_event_cursor WHERE id = 1';

const SAVE_CURSOR_SQL = `
  INSERT OR REPLACE INTO contract_event_cursor (id, cursor, last_ledger, updated_at)
  VALUES (1, ?, ?, CURRENT_TIMESTAMP)
`;

// Circuit breaker defaults
const CB_DEFAULTS = {
  failureThreshold: 5,
  resetTimeoutMs: 60_000,
  halfOpenMax: 1,
};

// Error categories for structured logging
export const ErrorCategory = {
  RPC_CONNECTION: 'rpc_connection',
  RPC_RESPONSE: 'rpc_response',
  PARSE: 'parse',
  DATABASE: 'database',
  HANDLER: 'handler',
  CURSOR: 'cursor',
  UNKNOWN: 'unknown',
};

function categorizeError(error) {
  const msg = (error?.message ?? '').toLowerCase();
  const code = error?.code;

  if (code === 'ECONNREFUSED' || code === 'ENOTFOUND' || code === 'ETIMEOUT') {
    return ErrorCategory.RPC_CONNECTION;
  }
  if (
    msg.includes('fetch') ||
    msg.includes('network') ||
    msg.includes('timeout')
  ) {
    return ErrorCategory.RPC_CONNECTION;
  }
  if (msg.includes('parse') || msg.includes('xdr') || msg.includes('decode')) {
    return ErrorCategory.PARSE;
  }
  if (
    msg.includes('database') ||
    msg.includes('sqlite') ||
    msg.includes('sql')
  ) {
    return ErrorCategory.DATABASE;
  }
  if (msg.includes('handler')) {
    return ErrorCategory.HANDLER;
  }
  return ErrorCategory.UNKNOWN;
}

class ContractEventIndexer {
  constructor() {
    const { rpcUrl, contractIds, pollIntervalMs } = config.indexer;
    this._rpcUrl = rpcUrl;
    this._contractIds = contractIds;
    this._intervalMs = pollIntervalMs;
    this._db = new DatabaseService();
    this._timer = null;

    // Circuit breaker state
    this._cb = {
      state: 'closed', // 'closed' | 'open' | 'half-open'
      failures: 0,
      lastFailureTime: 0,
      halfOpenAttempts: 0,
      ...CB_DEFAULTS,
    };

    // Health / status tracking
    this._status = {
      running: false,
      lastPollAt: null,
      lastSuccessAt: null,
      lastError: null,
      eventsProcessed: 0,
      eventsFailed: 0,
      consecutiveErrors: 0,
    };

    this._errorListeners = [];
  }

  /**
   * Subscribe to indexer error events for monitoring / alerting.
   * Returns an unsubscribe function.
   */
  onError(listener) {
    this._errorListeners.push(listener);
    return () => {
      this._errorListeners = this._errorListeners.filter((l) => l !== listener);
    };
  }

  /**
   * Returns a snapshot of the indexer's health status.
   */
  getStatus() {
    return {
      ...this._status,
      circuitBreaker: { ...this._cb },
    };
  }

  async start() {
    await this._db.connect();
    this._status.running = true;
    const t = setInterval(
      () =>
        this._poll().catch((e) => {
          this._recordError(e, ErrorCategory.UNKNOWN);
        }),
      this._intervalMs
    );
    t.unref();
    this._timer = t;
    // Fire once immediately without awaiting so startup is non-blocking
    this._poll().catch((e) => {
      this._recordError(e, ErrorCategory.UNKNOWN);
    });
  }

  stop() {
    if (this._timer) {
      clearInterval(this._timer);
      this._timer = null;
    }
    this._status.running = false;
  }

  // ── Circuit breaker ─────────────────────────────────────────────────────────

  _isCircuitOpen() {
    if (this._cb.state === 'open') {
      const elapsed = Date.now() - this._cb.lastFailureTime;
      if (elapsed >= this._cb.resetTimeoutMs) {
        this._cb.state = 'half-open';
        this._cb.halfOpenAttempts = 0;
        return false;
      }
      return true;
    }
    return false;
  }

  _recordFailure() {
    this._cb.failures++;
    this._cb.lastFailureTime = Date.now();
    if (this._cb.failures >= this._cb.failureThreshold) {
      this._cb.state = 'open';
    }
  }

  _recordSuccess() {
    this._cb.failures = 0;
    this._cb.halfOpenAttempts = 0;
    this._cb.state = 'closed';
  }

  // ── Structured error reporting ──────────────────────────────────────────────

  _recordError(error, category = null) {
    const cat = category || categorizeError(error);
    const entry = {
      timestamp: new Date().toISOString(),
      category: cat,
      message: error?.message ?? String(error),
      code: error?.code,
    };

    this._status.lastError = entry;
    this._status.consecutiveErrors++;

    if (
      cat === ErrorCategory.RPC_CONNECTION ||
      cat === ErrorCategory.RPC_RESPONSE
    ) {
      this._recordFailure();
    }

    for (const listener of this._errorListeners) {
      try {
        listener(entry);
      } catch {
        // Listener errors must not crash the indexer
      }
    }

    console.error(
      `[Indexer] ${cat} error (consecutive: ${this._status.consecutiveErrors}):`,
      error?.message ?? error
    );
  }

  // ── Cursor management ───────────────────────────────────────────────────────

  async _loadCursor() {
    try {
      const row = await this._db.get(LOAD_CURSOR_SQL);
      return row ? row.last_ledger : 0;
    } catch (e) {
      this._recordError(e, ErrorCategory.CURSOR);
      return 0;
    }
  }

  async _saveCursor(lastLedger) {
    try {
      await this._db.run(SAVE_CURSOR_SQL, [String(lastLedger), lastLedger]);
    } catch (e) {
      this._recordError(e, ErrorCategory.DATABASE);
    }
  }

  // ── Core poll loop ──────────────────────────────────────────────────────────

  async _poll() {
    if (!this._contractIds.length) return;
    if (this._isCircuitOpen()) return;

    this._status.lastPollAt = new Date().toISOString();

    // Dynamic import defers the SDK load until the first poll
    const sdk = await import('@stellar/stellar-sdk');
    const SorobanRpc = sdk.rpc || sdk.SorobanRpc;
    const rpcServer = new SorobanRpc.Server(this._rpcUrl);
    const startLedger = await this._loadCursor();

    let result;
    try {
      result = await rpcServer.getEvents({
        startLedger,
        filters: [{ type: 'contract', contractIds: this._contractIds }],
        limit: 200,
      });
    } catch (e) {
      const cat = categorizeError(e);
      this._recordError(e, cat);
      if (this._cb.state === 'half-open') {
        this._cb.halfOpenAttempts++;
        if (this._cb.halfOpenAttempts > this._cb.halfOpenMax) {
          this._cb.state = 'open';
          this._cb.lastFailureTime = Date.now();
        }
      }
      return;
    }

    const events = result.events ?? [];
    let latestLedger = startLedger;
    let processed = 0;
    let failed = 0;

    for (const raw of events) {
      try {
        await this._processEvent(raw);
        processed++;
      } catch (e) {
        failed++;
        this._recordError(e, e._category || categorizeError(e));
      }
      if (raw.ledger > latestLedger) latestLedger = raw.ledger;
    }

    if (latestLedger > startLedger) {
      await this._saveCursor(latestLedger);
    }

    // Success — reset circuit breaker and consecutive error count
    this._recordSuccess();
    this._status.lastSuccessAt = new Date().toISOString();
    this._status.consecutiveErrors = 0;
    this._status.eventsProcessed += processed;
    this._status.eventsFailed += failed;
  }

  async _processEvent(raw) {
    const parsed = parseEvent(raw);
    await this._db.run(INSERT_EVENT_SQL, [
      parsed.contractId,
      parsed.ledgerSequence,
      JSON.stringify(parsed.topics),
      JSON.stringify(parsed.value),
      parsed.rawXdr,
      parsed.eventType,
    ]);
    dispatchEvent(parsed);
  }
}

export const contractEventIndexer = new ContractEventIndexer();
export default contractEventIndexer;
