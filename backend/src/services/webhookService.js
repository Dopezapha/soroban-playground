// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT
//
// Real-Time Event Webhook Notification Dispatcher with HMAC Signatures
//
// Dispatches on-chain Soroban contract events to external user webhooks with:
//   - HMAC-SHA256 request signing (GitHub-compatible `sha256=<hex>` format)
//   - Exponential back-off retry (up to MAX_ATTEMPTS attempts)
//   - Per-subscription circuit breaker (pauses delivery after N consecutive
//     failures to protect downstream services from thundering-herd retries)
//   - Tenant-isolated subscriptions with wildcard event routing
//   - Delivery log with full request/response history
//   - Configurable request timeout and body size cap
//
// Architecture
// ────────────
//   webhookService.js    ← this file  (subscription CRUD + delivery engine)
//   webhookDispatcher.js             (polling loop that calls processPendingDeliveries)
//   webhookUtils.js                  (pure crypto / timing helpers, imported below)

import crypto from 'crypto';
import http from 'http';
import https from 'https';
import { getDatabase } from '../database/connection.js';
import {
  generateSignature,
  verifySignature,
  buildDeliveryHeaders,
  nextAttemptAt,
  MAX_ATTEMPTS,
  TIMEOUT_MS,
} from './webhookUtils.js';

// Re-export so callers only need to import from this file
export { generateSignature, verifySignature } from './webhookUtils.js';

// ── Constants ─────────────────────────────────────────────────────────────────

/** After this many consecutive failures a subscription enters the PAUSED state
 *  and stops receiving deliveries until manually re-enabled. */
const CIRCUIT_BREAKER_THRESHOLD = 10;

/** Maximum response body size we store in the delivery log (bytes). */
const MAX_RESPONSE_BODY_BYTES = 2_048;

/** Maximum webhook request payload we will POST (bytes).  Larger payloads are
 *  truncated to prevent memory exhaustion on either side. */
const MAX_PAYLOAD_BYTES = 512 * 1024; // 512 KiB

// ── Helpers ───────────────────────────────────────────────────────────────────

function newId() {
  return crypto.randomBytes(12).toString('hex');
}

/**
 * POST `body` (JSON string) to `url` with the supplied headers.
 * Resolves with `{ status, body }` or rejects on network error / timeout.
 *
 * @param {string} url
 * @param {string} body
 * @param {Record<string,string>} headers
 * @returns {Promise<{status:number, body:string}>}
 */
function postJson(url, body, headers) {
  return new Promise((resolve, reject) => {
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      return reject(new Error(`Invalid webhook URL: ${url}`));
    }

    const isHttps = parsed.protocol === 'https:';
    const client = isHttps ? https : http;

    // Truncate oversized payloads
    const payloadBuffer = Buffer.from(body, 'utf8');
    const cappedPayload =
      payloadBuffer.length > MAX_PAYLOAD_BYTES
        ? payloadBuffer.slice(0, MAX_PAYLOAD_BYTES)
        : payloadBuffer;

    const options = {
      hostname: parsed.hostname,
      port: parsed.port || (isHttps ? 443 : 80),
      path: parsed.pathname + parsed.search,
      method: 'POST',
      headers: {
        ...headers,
        'Content-Type': 'application/json',
        'Content-Length': cappedPayload.length,
        'User-Agent': 'SorobanPlayground-Webhooks/1.0',
      },
    };

    const req = client.request(options, (res) => {
      let text = '';
      let bytesRead = 0;

      res.on('data', (chunk) => {
        bytesRead += chunk.length;
        if (bytesRead <= MAX_RESPONSE_BODY_BYTES) {
          text += chunk;
        } else {
          res.destroy();
        }
      });

      res.on('end', () =>
        resolve({
          status: res.statusCode,
          body: text.slice(0, MAX_RESPONSE_BODY_BYTES),
        })
      );
    });

    req.setTimeout(TIMEOUT_MS, () =>
      req.destroy(new Error(`Webhook request timed out after ${TIMEOUT_MS} ms`))
    );

    req.on('error', reject);
    req.write(cappedPayload);
    req.end();
  });
}

// ── Subscription management ───────────────────────────────────────────────────

/**
 * Create a new webhook subscription for a tenant.
 *
 * @param {object} params
 * @param {string}   params.tenantId    - Owning tenant / organization ID.
 * @param {string}   params.url         - Target URL for delivery.
 * @param {string[]} [params.events]    - Event types to subscribe to.
 *                                        Use `['*']` for all events.
 * @param {string}   [params.secret]    - HMAC signing secret.  Auto-generated
 *                                        if not supplied.
 * @param {string}   [params.description] - Human-readable label.
 * @returns {Promise<object>}           The created subscription record.
 */
export async function createSubscription({
  tenantId,
  url,
  events = [],
  secret,
  description = '',
}) {
  if (!url) throw new Error('url is required');
  if (!tenantId) throw new Error('tenantId is required');

  // Validate URL
  try {
    new URL(url);
  } catch {
    throw new Error(`Invalid webhook URL: ${url}`);
  }

  const finalSecret = secret || crypto.randomBytes(32).toString('hex');
  const db = getDatabase();
  const id = newId();
  const eventsJson = JSON.stringify(Array.isArray(events) ? events : [events]);

  await db.run(
    `INSERT INTO webhook_subscriptions
       (id, tenant_id, url, events, secret, description, active, consecutive_failures, created_at, updated_at)
     VALUES
       (?, ?, ?, ?, ?, ?, 1, 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)`,
    [id, tenantId, url, eventsJson, finalSecret, description]
  );

  return db.get(
    `SELECT id, tenant_id, url, events, secret, description, active,
            consecutive_failures, created_at, updated_at
     FROM webhook_subscriptions
     WHERE id = ? AND tenant_id = ?`,
    [id, tenantId]
  );
}

/**
 * List all webhook subscriptions for a tenant.
 *
 * @param {string} tenantId
 * @returns {Promise<object[]>}
 */
export async function listSubscriptions(tenantId) {
  if (!tenantId) throw new Error('tenantId is required');
  const db = getDatabase();
  const rows = await db.all(
    `SELECT id, tenant_id, url, events, description, active,
            consecutive_failures, created_at, updated_at
     FROM webhook_subscriptions
     WHERE tenant_id = ?
     ORDER BY created_at DESC`,
    [tenantId]
  );
  return rows.map((r) => ({ ...r, events: JSON.parse(r.events) }));
}

/**
 * Update a subscription's URL, event list, or active state.
 *
 * @param {string} id
 * @param {string} tenantId
 * @param {object} updates  - `url`, `events`, `active`, `description`
 * @returns {Promise<boolean>}  true if the record was found and updated.
 */
export async function updateSubscription(id, tenantId, updates) {
  if (!tenantId) throw new Error('tenantId is required');
  const db = getDatabase();

  const fields = [];
  const values = [];

  if (updates.url !== undefined) {
    try {
      new URL(updates.url);
    } catch {
      throw new Error(`Invalid URL: ${updates.url}`);
    }
    fields.push('url = ?');
    values.push(updates.url);
  }
  if (updates.events !== undefined) {
    fields.push('events = ?');
    values.push(
      JSON.stringify(
        Array.isArray(updates.events) ? updates.events : [updates.events]
      )
    );
  }
  if (updates.active !== undefined) {
    fields.push('active = ?');
    fields.push('consecutive_failures = 0'); // reset circuit breaker on re-enable
    values.push(updates.active ? 1 : 0);
  }
  if (updates.description !== undefined) {
    fields.push('description = ?');
    values.push(updates.description);
  }

  if (fields.length === 0) return false;

  fields.push('updated_at = CURRENT_TIMESTAMP');
  values.push(id, tenantId);

  const { changes } = await db.run(
    `UPDATE webhook_subscriptions SET ${fields.join(', ')} WHERE id = ? AND tenant_id = ?`,
    values
  );
  return changes > 0;
}

/**
 * Delete a webhook subscription (and its associated delivery log entries).
 *
 * @param {string} id
 * @param {string} tenantId
 * @returns {Promise<boolean>}
 */
export async function deleteSubscription(id, tenantId) {
  if (!tenantId) throw new Error('tenantId is required');
  const db = getDatabase();
  await db.run(
    'DELETE FROM webhook_deliveries WHERE subscription_id = ? AND tenant_id = ?',
    [id, tenantId]
  );
  const { changes } = await db.run(
    'DELETE FROM webhook_subscriptions WHERE id = ? AND tenant_id = ?',
    [id, tenantId]
  );
  return changes > 0;
}

// ── On-chain event dispatching ────────────────────────────────────────────────

/**
 * Enqueue a delivery job for every active subscription that listens to `eventType`.
 *
 * This is the primary integration point for the contract event indexer:
 *
 * ```js
 * // Inside contractEventIndexer.js (or similar)
 * await enqueueEvent('contract.deployed', {
 *   contractId: 'C...',
 *   ledger: 12345,
 *   tx: 'a1b2...',
 * }, tenantId);
 * ```
 *
 * @param {string} eventType    - Event type string, e.g. `contract.deployed`.
 * @param {object} payload      - Arbitrary serialisable event data.
 * @param {string} tenantId     - Owning tenant.
 * @returns {Promise<string[]>} - Array of created delivery IDs.
 */
export async function enqueueEvent(eventType, payload, tenantId) {
  if (!tenantId) throw new Error('tenantId is required');
  const db = getDatabase();

  // Only dispatch to active subscriptions whose circuit breaker has not tripped
  const subs = await db.all(
    `SELECT id, events
     FROM webhook_subscriptions
     WHERE tenant_id = ? AND active = 1
       AND consecutive_failures < ?`,
    [tenantId, CIRCUIT_BREAKER_THRESHOLD]
  );

  const deliveryIds = [];
  for (const sub of subs) {
    const subscribedEvents = JSON.parse(sub.events);

    // Wildcard subscription or explicit match
    const matches =
      subscribedEvents.length === 0 ||
      subscribedEvents.includes('*') ||
      subscribedEvents.includes(eventType);

    if (!matches) continue;

    const id = newId();
    await db.run(
      `INSERT INTO webhook_deliveries
         (id, tenant_id, subscription_id, event_type, payload, status, attempt, next_attempt_at, created_at)
       VALUES
         (?, ?, ?, ?, ?, 'pending', 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)`,
      [id, tenantId, sub.id, eventType, JSON.stringify(payload)]
    );
    deliveryIds.push(id);
  }

  return deliveryIds;
}

// ── Background delivery engine ────────────────────────────────────────────────

/**
 * Process all pending and retrying deliveries that are due.
 * Called periodically by `webhookDispatcher.js`.
 *
 * The function fetches up to 50 due deliveries per invocation to bound memory
 * usage.  For high-throughput deployments, consider running multiple dispatcher
 * instances with database-level row locking.
 *
 * @returns {Promise<number>} Number of deliveries processed.
 */
export async function processPendingDeliveries() {
  const db = getDatabase();

  const due = await db.all(
    `SELECT d.id,
            d.subscription_id,
            d.event_type,
            d.payload,
            d.attempt,
            s.url,
            s.secret,
            s.consecutive_failures
     FROM webhook_deliveries d
     JOIN webhook_subscriptions s ON s.id = d.subscription_id
     WHERE d.status IN ('pending', 'retrying')
       AND d.next_attempt_at <= CURRENT_TIMESTAMP
       AND s.active = 1
       AND s.consecutive_failures < ?
     LIMIT 50`,
    [CIRCUIT_BREAKER_THRESHOLD]
  );

  for (const row of due) {
    const attempt = row.attempt + 1;
    const payloadString = row.payload;

    // Build HMAC-signed delivery headers
    const headers = buildDeliveryHeaders(payloadString, row.secret, row.id);

    let result;
    try {
      result = await postJson(row.url, payloadString, headers);
    } catch (err) {
      result = {
        status: null,
        body: String(err.message).slice(0, MAX_RESPONSE_BODY_BYTES),
      };
    }

    const success =
      typeof result.status === 'number' &&
      result.status >= 200 &&
      result.status < 300;

    if (success) {
      // Delivery succeeded — reset the consecutive failure counter
      await db.run(
        `UPDATE webhook_deliveries
         SET status = 'success',
             attempt = ?,
             response_status = ?,
             response_body = ?,
             delivered_at = CURRENT_TIMESTAMP
         WHERE id = ?`,
        [attempt, result.status, result.body, row.id]
      );

      await db.run(
        `UPDATE webhook_subscriptions
         SET consecutive_failures = 0, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?`,
        [row.subscription_id]
      );
    } else if (attempt >= MAX_ATTEMPTS) {
      // Exhausted all retries — permanently mark as failed and increment circuit breaker
      await db.run(
        `UPDATE webhook_deliveries
         SET status = 'failed',
             attempt = ?,
             response_status = ?,
             response_body = ?
         WHERE id = ?`,
        [attempt, result.status, result.body, row.id]
      );

      const newConsecutive = (row.consecutive_failures || 0) + 1;
      await db.run(
        `UPDATE webhook_subscriptions
         SET consecutive_failures = ?,
             active = CASE WHEN ? >= ? THEN 0 ELSE active END,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?`,
        [
          newConsecutive,
          newConsecutive,
          CIRCUIT_BREAKER_THRESHOLD,
          row.subscription_id,
        ]
      );

      if (newConsecutive >= CIRCUIT_BREAKER_THRESHOLD) {
        console.warn(
          `[webhookService] Circuit breaker tripped for subscription ${row.subscription_id} ` +
            `after ${newConsecutive} consecutive failures. Subscription paused.`
        );
      }
    } else {
      // Temporary failure — schedule exponential back-off retry
      await db.run(
        `UPDATE webhook_deliveries
         SET status = 'retrying',
             attempt = ?,
             response_status = ?,
             response_body = ?,
             next_attempt_at = ?
         WHERE id = ?`,
        [attempt, result.status, result.body, nextAttemptAt(attempt), row.id]
      );

      const newConsecutive = (row.consecutive_failures || 0) + 1;
      await db.run(
        `UPDATE webhook_subscriptions
         SET consecutive_failures = ?, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?`,
        [newConsecutive, row.subscription_id]
      );
    }
  }

  return due.length;
}

// ── Delivery history ──────────────────────────────────────────────────────────

/**
 * Retrieve the delivery log for a tenant (optionally filtered by subscription).
 *
 * @param {string}  tenantId
 * @param {string|null} [subscriptionId]
 * @param {number}  [limit=50]
 * @returns {Promise<object[]>}
 */
export async function listDeliveries(
  tenantId,
  subscriptionId = null,
  limit = 50
) {
  if (!tenantId) throw new Error('tenantId is required');
  const db = getDatabase();

  const params = [tenantId];
  let where = 'WHERE d.tenant_id = ?';

  if (subscriptionId) {
    where += ' AND d.subscription_id = ?';
    params.push(subscriptionId);
  }

  params.push(limit);

  return db.all(
    `SELECT d.id,
            d.subscription_id,
            d.event_type,
            d.status,
            d.attempt,
            d.response_status,
            d.delivered_at,
            d.created_at
     FROM webhook_deliveries d
     ${where}
     ORDER BY d.created_at DESC
     LIMIT ?`,
    params
  );
}

/**
 * Manually replay a specific delivery (useful for debugging or recovering from
 * transient endpoint outages).
 *
 * @param {string} deliveryId
 * @param {string} tenantId
 * @returns {Promise<{success: boolean, status: number|null, body: string}>}
 */
export async function replayDelivery(deliveryId, tenantId) {
  if (!tenantId) throw new Error('tenantId is required');
  const db = getDatabase();

  const row = await db.get(
    `SELECT d.id, d.payload, d.attempt,
            s.url, s.secret, s.tenant_id AS sub_tenant_id
     FROM webhook_deliveries d
     JOIN webhook_subscriptions s ON s.id = d.subscription_id
     WHERE d.id = ? AND d.tenant_id = ?`,
    [deliveryId, tenantId]
  );

  if (!row) throw new Error(`Delivery ${deliveryId} not found`);

  const headers = buildDeliveryHeaders(row.payload, row.secret, row.id);
  let result;
  try {
    result = await postJson(row.url, row.payload, headers);
  } catch (err) {
    result = { status: null, body: err.message };
  }

  const success =
    typeof result.status === 'number' &&
    result.status >= 200 &&
    result.status < 300;

  const attempt = row.attempt + 1;
  if (success) {
    await db.run(
      `UPDATE webhook_deliveries
       SET status = 'success', attempt = ?, response_status = ?,
           response_body = ?, delivered_at = CURRENT_TIMESTAMP
       WHERE id = ?`,
      [attempt, result.status, result.body, deliveryId]
    );
  }

  return { success, status: result.status, body: result.body };
}

// ── HMAC verification helper (for endpoint owners) ────────────────────────────

/**
 * Verify an inbound webhook request (for use by consumers of this webhook
 * system who want to validate that requests genuinely originate from the
 * Soroban Playground dispatcher).
 *
 * @param {string} rawBody    - Raw request body string.
 * @param {string} secret     - Subscription secret.
 * @param {string} signature  - Value of the `X-Soroban-Signature` header.
 * @returns {boolean}
 */
export function verifyInboundSignature(rawBody, secret, signature) {
  if (!signature || !secret) return false;
  return verifySignature(rawBody, secret, signature);
}

export default {
  createSubscription,
  listSubscriptions,
  updateSubscription,
  deleteSubscription,
  enqueueEvent,
  processPendingDeliveries,
  listDeliveries,
  replayDelivery,
  verifyInboundSignature,
  generateSignature,
  verifySignature,
};
