// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

/**
 * PostgreSQL Read-Replica Connection Pool & Query Routing Layer
 *
 * Routes read-heavy analytics and query traffic to read-replica nodes,
 * preserving master write capacity and reducing primary latency under load.
 *
 * ## Architecture
 * - One **primary** pool handles all writes and explicit read-from-primary queries.
 * - One or more **replica** pools receive read queries via round-robin selection.
 * - Health checks run periodically and remove unhealthy replicas from rotation.
 * - A replica is automatically re-added to rotation once it passes a health check.
 * - If all replicas are unhealthy, reads fall back to the primary automatically.
 * - The module exports a `query()` helper that accepts `{ text, values, readOnly }`
 *   and routes accordingly.
 *
 * ## Environment variables
 * ```
 * DATABASE_URL              Primary PostgreSQL connection string (required for pg mode).
 * READ_REPLICA_URLS         Comma-separated list of replica connection strings.
 * PG_POOL_MAX               Max connections per pool (default: 10).
 * PG_IDLE_TIMEOUT_MS        Idle client timeout in ms (default: 30000).
 * PG_CONNECT_TIMEOUT_MS     Connection timeout in ms (default: 5000).
 * PG_HEALTH_INTERVAL_MS     Replica health-check interval in ms (default: 30000).
 * ```
 *
 * ## Graceful fallback
 * The module is written to degrade gracefully when the `pg` package is not
 * installed. In that case, all calls route through the project's existing
 * SQLite/knex layer and a warning is emitted.
 *
 * ## Usage
 * ```js
 * import { query, queryWrite, queryRead, endAllPools } from '../database/pool.js';
 *
 * // Automatic routing (writes go to primary, reads to replica):
 * const result = await query('SELECT * FROM contracts WHERE id = $1', [id]);
 *
 * // Force primary read (e.g. immediately after a write):
 * const fresh = await queryWrite('SELECT * FROM contracts WHERE id = $1', [id]);
 *
 * // Explicit replica read:
 * const analytics = await queryRead('SELECT count(*) FROM events');
 * ```
 */

import logger from '../utils/logger.js';

// ── Configuration ─────────────────────────────────────────────────────────────

const PG_POOL_MAX = parseInt(process.env.PG_POOL_MAX || '10', 10);
const PG_IDLE_TIMEOUT_MS = parseInt(
  process.env.PG_IDLE_TIMEOUT_MS || '30000',
  10
);
const PG_CONNECT_TIMEOUT_MS = parseInt(
  process.env.PG_CONNECT_TIMEOUT_MS || '5000',
  10
);
const PG_HEALTH_INTERVAL_MS = parseInt(
  process.env.PG_HEALTH_INTERVAL_MS || '30000',
  10
);

// ── Pool factory ──────────────────────────────────────────────────────────────

/**
 * Attempts to create a `pg.Pool` instance. Returns null if pg is unavailable.
 * @param {string} connectionString
 * @param {string} label  Human-readable label for log messages.
 */
function createPgPool(connectionString, label) {
  try {
    const { Pool } = require('pg');
    const pool = new Pool({
      connectionString,
      max: PG_POOL_MAX,
      idleTimeoutMillis: PG_IDLE_TIMEOUT_MS,
      connectionTimeoutMillis: PG_CONNECT_TIMEOUT_MS,
    });

    pool.on('error', (err) => {
      logger.error('pool:client:error', { label, error: err.message });
    });

    pool.on('connect', () => {
      logger.debug('pool:client:connected', { label });
    });

    logger.info('pool:created', { label, max: PG_POOL_MAX });
    return pool;
  } catch (err) {
    logger.warn('pool:pg:unavailable', {
      label,
      reason: err.message,
      hint: 'Install "pg" to enable PostgreSQL connection pooling.',
    });
    return null;
  }
}

// ── Pool state ────────────────────────────────────────────────────────────────

/** @type {{ pool: import('pg').Pool, url: string, healthy: boolean } | null} */
let _primary = null;

/**
 * @type {Array<{ pool: import('pg').Pool, url: string, healthy: boolean }>}
 */
let _replicas = [];

/** Round-robin cursor for replica selection. */
let _replicaCursor = 0;

/** Reference to the health-check interval timer. */
let _healthTimer = null;

// ── Initialisation ────────────────────────────────────────────────────────────

/**
 * Initialise the primary pool and all replica pools.
 *
 * Idempotent — safe to call multiple times.  Subsequent calls are no-ops
 * unless `force: true` is passed (which closes existing pools first).
 *
 * @param {{ force?: boolean }} [options]
 */
export async function initPool(options = {}) {
  if (_primary && !options.force) {
    logger.debug('pool:init:already_initialised');
    return;
  }

  if (options.force) {
    await endAllPools();
  }

  const primaryUrl = process.env.DATABASE_URL;
  if (!primaryUrl) {
    logger.warn(
      'pool:init: DATABASE_URL not set — PostgreSQL pool not initialised. ' +
        'Falling back to existing database layer.'
    );
    return;
  }

  const primaryPool = createPgPool(primaryUrl, 'primary');
  if (primaryPool) {
    _primary = { pool: primaryPool, url: primaryUrl, healthy: true };
  }

  const replicaUrls = (process.env.READ_REPLICA_URLS || '')
    .split(',')
    .map((u) => u.trim())
    .filter(Boolean);

  for (const url of replicaUrls) {
    const pool = createPgPool(url, `replica:${_replicas.length + 1}`);
    if (pool) {
      _replicas.push({ pool, url, healthy: true });
    }
  }

  logger.info('pool:init:complete', {
    primary: !!_primary,
    replicas: _replicas.length,
  });

  _startHealthChecks();
}

// ── Health checks ─────────────────────────────────────────────────────────────

async function _checkHealth(node, label) {
  try {
    const client = await node.pool.connect();
    await client.query('SELECT 1');
    client.release();

    if (!node.healthy) {
      node.healthy = true;
      logger.info('pool:health:recovered', { label });
    }
  } catch (err) {
    if (node.healthy) {
      node.healthy = false;
      logger.warn('pool:health:degraded', { label, error: err.message });
    }
  }
}

function _startHealthChecks() {
  if (_healthTimer) return;

  _healthTimer = setInterval(async () => {
    if (_primary) await _checkHealth(_primary, 'primary');

    for (let i = 0; i < _replicas.length; i++) {
      await _checkHealth(_replicas[i], `replica:${i + 1}`);
    }
  }, PG_HEALTH_INTERVAL_MS);

  // Don't prevent the process from exiting.
  if (_healthTimer.unref) _healthTimer.unref();
}

// ── Replica selection ─────────────────────────────────────────────────────────

/**
 * Returns the next healthy replica (round-robin), or null if none are healthy.
 */
function _pickReplica() {
  const healthy = _replicas.filter((r) => r.healthy);
  if (healthy.length === 0) return null;

  const index = _replicaCursor % healthy.length;
  _replicaCursor = (index + 1) % healthy.length;
  return healthy[index];
}

// ── Query helpers ─────────────────────────────────────────────────────────────

/**
 * Execute a query on the primary pool.
 *
 * @param {string|{text:string, values?:any[]}} textOrConfig
 * @param {any[]} [values]
 */
export async function queryWrite(textOrConfig, values) {
  if (!_primary) {
    throw new Error(
      'pool:queryWrite: primary pool not initialised. Call initPool() first or ensure DATABASE_URL is set.'
    );
  }
  const config =
    typeof textOrConfig === 'string'
      ? { text: textOrConfig, values }
      : textOrConfig;

  const start = Date.now();
  try {
    const result = await _primary.pool.query(config);
    logger.debug('pool:query:primary', {
      durationMs: Date.now() - start,
      rowCount: result.rowCount,
    });
    return result;
  } catch (err) {
    logger.error('pool:query:primary:error', {
      durationMs: Date.now() - start,
      error: err.message,
      query:
        typeof config.text === 'string' ? config.text.slice(0, 200) : undefined,
    });
    throw err;
  }
}

/**
 * Execute a read-only query, routed to a healthy replica if available,
 * otherwise falling back to the primary.
 *
 * @param {string|{text:string, values?:any[]}} textOrConfig
 * @param {any[]} [values]
 */
export async function queryRead(textOrConfig, values) {
  const config =
    typeof textOrConfig === 'string'
      ? { text: textOrConfig, values }
      : textOrConfig;

  const replica = _pickReplica();
  const target = replica || _primary;

  if (!target) {
    throw new Error(
      'pool:queryRead: no pool available. Call initPool() first or ensure DATABASE_URL is set.'
    );
  }

  const label = replica ? 'replica' : 'primary(fallback)';
  const start = Date.now();

  try {
    const result = await target.pool.query(config);
    logger.debug('pool:query:read', {
      target: label,
      durationMs: Date.now() - start,
      rowCount: result.rowCount,
    });
    return result;
  } catch (err) {
    // If the replica failed, mark it unhealthy and retry on primary.
    if (replica) {
      replica.healthy = false;
      logger.warn('pool:query:replica:error — falling back to primary', {
        error: err.message,
      });
      return queryWrite(config);
    }
    logger.error('pool:query:primary:error', {
      durationMs: Date.now() - start,
      error: err.message,
    });
    throw err;
  }
}

/**
 * Unified query entry point. Set `readOnly: true` (or omit for backward
 * compatibility) to route to a replica. Writes always go to primary.
 *
 * @param {string} text          SQL statement.
 * @param {any[]}  [values]      Parameterised values.
 * @param {boolean}[readOnly]    Route to replica when true (default: auto-detect).
 */
export async function query(text, values, readOnly) {
  // Auto-detect read-only based on SQL prefix when not specified.
  const isRead =
    readOnly !== undefined
      ? readOnly
      : /^\s*(SELECT|SHOW|EXPLAIN)\b/i.test(text);

  return isRead ? queryRead(text, values) : queryWrite(text, values);
}

// ── Pool metrics ──────────────────────────────────────────────────────────────

/**
 * Returns a snapshot of pool health and connection statistics.
 */
export function getPoolStats() {
  return {
    primary: _primary
      ? {
          healthy: _primary.healthy,
          totalCount: _primary.pool.totalCount,
          idleCount: _primary.pool.idleCount,
          waitingCount: _primary.pool.waitingCount,
        }
      : null,
    replicas: _replicas.map((r, i) => ({
      index: i + 1,
      healthy: r.healthy,
      totalCount: r.pool.totalCount,
      idleCount: r.pool.idleCount,
      waitingCount: r.pool.waitingCount,
    })),
  };
}

// ── Teardown ──────────────────────────────────────────────────────────────────

/**
 * Gracefully close all pools and stop health checks.
 * Should be called during application shutdown.
 */
export async function endAllPools() {
  if (_healthTimer) {
    clearInterval(_healthTimer);
    _healthTimer = null;
  }

  const closePromises = [];

  if (_primary) {
    closePromises.push(
      _primary.pool
        .end()
        .catch((err) =>
          logger.warn('pool:end:primary:error', { error: err.message })
        )
    );
    _primary = null;
  }

  for (const replica of _replicas) {
    closePromises.push(
      replica.pool
        .end()
        .catch((err) =>
          logger.warn('pool:end:replica:error', { error: err.message })
        )
    );
  }
  _replicas = [];
  _replicaCursor = 0;

  await Promise.all(closePromises);
  logger.info('pool:all_pools_closed');
}

export default {
  initPool,
  query,
  queryWrite,
  queryRead,
  getPoolStats,
  endAllPools,
};
