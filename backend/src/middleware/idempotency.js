// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT
//
// API Request De-Duplication & Idempotency Key Middleware
//
// Prevents double-submission of contract deployments by enforcing idempotency
// keys on state-mutating endpoints.  Behaviour:
//
//   1. Client sends `Idempotency-Key: <uuid>` header on a POST/PUT/PATCH request.
//   2. Middleware checks Redis (falling back to an in-memory LRU when Redis is
//      unavailable) for an existing result keyed by `<tenantId>:<key>`.
//   3. If a cached result exists and the request has completed, it is replayed
//      immediately with the original status code and body.
//   4. If a record exists but the request is still "in-flight" (concurrent
//      duplicate), a 409 Conflict is returned.
//   5. On a new key a placeholder is written, the request is processed, and
//      the final response is stored before being sent to the client.
//
// Security notes:
//   - Keys are namespaced by tenant to prevent cross-tenant replay attacks.
//   - Max key age is configurable (default 24 h).
//   - The stored response body is capped to avoid unbounded Redis growth.
//   - The middleware is opt-in; routes must explicitly apply it.

import crypto from 'crypto';
import { LRUCache } from 'lru-cache';

// ── Constants ─────────────────────────────────────────────────────────────────

const IDEMPOTENCY_HEADER = 'idempotency-key';
const DEFAULT_TTL_SECONDS = 60 * 60 * 24; // 24 hours
const INFLIGHT_SENTINEL = '__inflight__';
const MAX_BODY_BYTES = 64 * 1024; // 64 KiB response body cap
const REDIS_PREFIX = 'idem:';

// In-memory fallback for environments without Redis
const memoryStore = new LRUCache({
  max: 10_000,
  ttl: DEFAULT_TTL_SECONDS * 1000,
});

// ── Storage helpers ───────────────────────────────────────────────────────────

/**
 * Attempt to import the shared redisService.  Returns null when unavailable so
 * the middleware degrades gracefully to the in-memory fallback.
 */
async function getRedis() {
  try {
    const mod = await import('../services/redisService.js');
    const svc = mod.default;
    // Only use Redis when it has an active connection
    if (svc?.client && !svc.isFallbackMode) {
      return svc.client;
    }
  } catch {
    // Redis not available – use memory fallback
  }
  return null;
}

/**
 * Read the idempotency record for `storeKey`.
 * Returns the parsed record object, or null if not found.
 */
async function readRecord(storeKey) {
  const redis = await getRedis();
  if (redis) {
    try {
      const raw = await redis.get(`${REDIS_PREFIX}${storeKey}`);
      return raw ? JSON.parse(raw) : null;
    } catch {
      // Fall through to memory store
    }
  }
  const hit = memoryStore.get(storeKey);
  return hit ?? null;
}

/**
 * Write an idempotency record.
 * @param {string} storeKey
 * @param {object} record
 * @param {number} [ttlSeconds]
 */
async function writeRecord(storeKey, record, ttlSeconds = DEFAULT_TTL_SECONDS) {
  const serialised = JSON.stringify(record);
  const redis = await getRedis();
  if (redis) {
    try {
      await redis.set(
        `${REDIS_PREFIX}${storeKey}`,
        serialised,
        'EX',
        ttlSeconds
      );
      return;
    } catch {
      // Fall through to memory store
    }
  }
  memoryStore.set(storeKey, record, { ttl: ttlSeconds * 1000 });
}

/**
 * Delete an idempotency record (used to clean up in-flight sentinels on error).
 */
async function deleteRecord(storeKey) {
  const redis = await getRedis();
  if (redis) {
    try {
      await redis.del(`${REDIS_PREFIX}${storeKey}`);
      return;
    } catch {
      // Fall through
    }
  }
  memoryStore.delete(storeKey);
}

// ── Key validation ────────────────────────────────────────────────────────────

/**
 * Returns true when `key` is a valid idempotency key (UUID v4 format or any
 * non-empty alphanumeric+hyphen+underscore string up to 128 chars).
 */
function isValidKey(key) {
  if (typeof key !== 'string') return false;
  const trimmed = key.trim();
  if (!trimmed || trimmed.length > 128) return false;
  // Accept UUID v4 or a broader safe-string pattern
  return /^[a-zA-Z0-9_\-]+$/.test(trimmed);
}

/**
 * Derive a namespaced store key from a tenant identifier and the raw
 * idempotency key supplied by the client.
 */
function buildStoreKey(tenantId, rawKey, method, path) {
  const hash = crypto
    .createHash('sha256')
    .update(`${tenantId}:${method}:${path}:${rawKey}`)
    .digest('hex')
    .slice(0, 32);
  return hash;
}

// ── Response interception ─────────────────────────────────────────────────────

/**
 * Wraps res.json / res.send so that the first response written is captured and
 * persisted under the idempotency key before being forwarded to the client.
 *
 * @param {import('express').Response} res
 * @param {string} storeKey
 * @param {number} [ttlSeconds]
 */
function interceptResponse(res, storeKey, ttlSeconds) {
  const originalJson = res.json.bind(res);
  const originalSend = res.send.bind(res);

  let intercepted = false;

  async function capture(body, sendFn, args) {
    if (intercepted) return sendFn(...args);
    intercepted = true;

    const statusCode = res.statusCode || 200;

    // Only cache successful or client-error responses; never cache 5xx.
    if (statusCode < 500) {
      let serialisedBody = '';
      try {
        serialisedBody = typeof body === 'string' ? body : JSON.stringify(body);
        if (Buffer.byteLength(serialisedBody) > MAX_BODY_BYTES) {
          serialisedBody = serialisedBody.slice(0, MAX_BODY_BYTES);
        }
      } catch {
        serialisedBody = '';
      }

      const record = {
        status: statusCode,
        body: serialisedBody,
        contentType: res.get('Content-Type') || 'application/json',
        completedAt: new Date().toISOString(),
      };

      // Fire-and-forget — do not delay the client response
      writeRecord(storeKey, record, ttlSeconds).catch(() => {});
    } else {
      // 5xx: remove the in-flight sentinel so the client can retry
      deleteRecord(storeKey).catch(() => {});
    }

    return sendFn(...args);
  }

  res.json = function (...args) {
    return capture(args[0], originalJson, args);
  };

  res.send = function (...args) {
    return capture(args[0], originalSend, args);
  };
}

// ── Middleware factory ────────────────────────────────────────────────────────

/**
 * Express middleware that enforces idempotency for state-mutating requests.
 *
 * @param {object} [options]
 * @param {number}  [options.ttlSeconds=86400]  - How long to cache results (seconds).
 * @param {boolean} [options.requireKey=false]  - When true, reject requests that
 *   don't supply an Idempotency-Key header (useful on critical endpoints like
 *   `/deploy`).
 * @param {boolean} [options.requireTenant=true] - When true, reject requests
 *   without a resolved tenant context (prevents key collisions across tenants).
 *
 * @returns {import('express').RequestHandler}
 *
 * @example
 * // Optional key (recommended default)
 * router.post('/deploy', idempotency(), deployHandler);
 *
 * // Mandatory key — any deploy without a key is rejected with 400
 * router.post('/deploy', idempotency({ requireKey: true }), deployHandler);
 */
export function idempotency({
  ttlSeconds = DEFAULT_TTL_SECONDS,
  requireKey = false,
  requireTenant = true,
} = {}) {
  return async (req, res, next) => {
    // Only apply to state-mutating methods
    const method = req.method?.toUpperCase();
    if (!['POST', 'PUT', 'PATCH', 'DELETE'].includes(method)) {
      return next();
    }

    const rawKey = req.headers[IDEMPOTENCY_HEADER];

    if (!rawKey) {
      if (requireKey) {
        return res.status(400).json({
          error: 'Idempotency-Key header is required for this endpoint',
          code: 'IDEMPOTENCY_KEY_REQUIRED',
        });
      }
      // No key supplied and not required — pass through normally
      return next();
    }

    if (!isValidKey(rawKey)) {
      return res.status(400).json({
        error:
          'Invalid Idempotency-Key. Must be 1-128 alphanumeric/hyphen/underscore characters.',
        code: 'IDEMPOTENCY_KEY_INVALID',
      });
    }

    // Resolve tenant for key namespacing
    const tenantId = req.tenant?.id || req.auth?.organizationId || 'global';
    if (requireTenant && tenantId === 'global') {
      return res.status(401).json({
        error:
          'Tenant context is required when using Idempotency-Key. Provide a valid API key or bearer token.',
        code: 'TENANT_REQUIRED',
      });
    }

    const storeKey = buildStoreKey(tenantId, rawKey, method, req.path);

    // ── Check for existing record ────────────────────────────────────────────
    let record;
    try {
      record = await readRecord(storeKey);
    } catch {
      // Storage error — degrade gracefully and let the request through
      return next();
    }

    if (record) {
      if (record === INFLIGHT_SENTINEL || record.status === undefined) {
        // Concurrent duplicate request
        return res.status(409).json({
          error:
            'A request with this Idempotency-Key is already being processed. Retry after the original request completes.',
          code: 'IDEMPOTENCY_REQUEST_IN_FLIGHT',
          idempotencyKey: rawKey,
        });
      }

      // Replay the cached response
      res.setHeader('Idempotency-Key', rawKey);
      res.setHeader('X-Idempotent-Replayed', 'true');
      res.setHeader('Content-Type', record.contentType || 'application/json');

      let parsedBody;
      try {
        parsedBody = JSON.parse(record.body);
      } catch {
        parsedBody = record.body;
      }

      return res.status(record.status).json(parsedBody);
    }

    // ── New key: mark as in-flight ───────────────────────────────────────────
    try {
      await writeRecord(storeKey, { status: undefined }, ttlSeconds);
    } catch {
      // If we can't write the sentinel, proceed without idempotency protection
      // rather than failing the request entirely.
      return next();
    }

    // Attach key metadata to the request for downstream logging / tracing
    req.idempotencyKey = rawKey;
    req.idempotencyStoreKey = storeKey;

    // Intercept the outgoing response to capture and store the result
    interceptResponse(res, storeKey, ttlSeconds);

    // Propagate any unhandled errors to clean up the in-flight sentinel
    const originalNext = next;
    const guardedNext = (err) => {
      if (err) {
        deleteRecord(storeKey).catch(() => {});
      }
      originalNext(err);
    };

    return guardedNext();
  };
}

/**
 * Convenience wrapper: a single pre-configured middleware instance suitable
 * for use on any route without additional options.
 *
 * @type {import('express').RequestHandler}
 */
export const idempotencyMiddleware = idempotency();

export default idempotency;
