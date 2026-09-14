// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

/**
 * Distributed Job Scheduling Engine with Redlock Mutual Exclusion
 *
 * Ensures that recurring cron maintenance jobs execute on exactly **one**
 * cluster replica at a time, even when multiple Node.js processes share the
 * same Redis instance.
 *
 * ## Design
 * - Each job is registered with a name, a cron expression, and a handler.
 * - Before the handler runs, the scheduler attempts to acquire a Redlock-style
 *   distributed lock stored in Redis under `scheduler:lock:<jobName>`.
 * - If the lock is acquired, the handler runs; the lock is released afterwards
 *   (or expires automatically after `lockTtlMs` to handle process crashes).
 * - If the lock cannot be acquired (another replica already holds it), the
 *   current replica skips this tick silently.
 * - The implementation degrades gracefully when Redis is unavailable: jobs
 *   still execute locally (single-replica mode) with a warning logged.
 *
 * ## Usage
 * ```js
 * import { registerJob, startScheduler, stopScheduler } from './scheduler.js';
 *
 * registerJob({
 *   name: 'ledger-sync',
 *   schedule: '* /5 * * * *',   // every 5 minutes
 *   handler: async () => { ... },
 *   lockTtlMs: 4 * 60 * 1000,   // lock expires in 4 min (< interval)
 * });
 *
 * startScheduler();
 * ```
 *
 * ## Environment variables
 * - `REDIS_URL`           – Redis connection string (default: redis://localhost:6379)
 * - `SCHEDULER_ENABLED`   – set to 'false' to disable all scheduled jobs
 * - `SCHEDULER_LOCK_TTL`  – default lock TTL in ms (default: 55000)
 */

import cron from 'node-cron';
import logger from '../utils/logger.js';

// ── Redis / Redlock helpers ───────────────────────────────────────────────────

/**
 * Attempts to load ioredis. Returns null if unavailable.
 */
function tryGetRedis() {
  try {
    const Redis = require('ioredis');
    const url = process.env.REDIS_URL || 'redis://localhost:6379';
    const client = new Redis(url, {
      lazyConnect: true,
      enableOfflineQueue: false,
      connectTimeout: 3000,
      maxRetriesPerRequest: 1,
    });
    client.on('error', (err) => {
      logger.warn('scheduler:redis:error', { error: err.message });
    });
    return client;
  } catch {
    return null;
  }
}

let _redis = null;
let _redisChecked = false;

function getRedis() {
  if (!_redisChecked) {
    _redis = tryGetRedis();
    _redisChecked = true;
    if (!_redis) {
      logger.warn(
        'scheduler: ioredis not available — running in single-replica mode (no distributed locking)'
      );
    }
  }
  return _redis;
}

/**
 * Acquire a Redlock-style lock.
 *
 * Uses a Lua script for atomic SET-if-not-exists + expiry so there is no
 * race between checking and setting the key.
 *
 * @param {string}  lockKey  Redis key for this lock.
 * @param {string}  token    Unique random value (identifies owner).
 * @param {number}  ttlMs    Lock expiry in milliseconds.
 * @returns {Promise<boolean>} true if the lock was acquired.
 */
async function acquireLock(lockKey, token, ttlMs) {
  const redis = getRedis();
  if (!redis) return true; // no Redis → always run (single-replica fallback)

  try {
    const result = await redis.set(lockKey, token, 'PX', ttlMs, 'NX');
    return result === 'OK';
  } catch (err) {
    logger.warn('scheduler:lock:acquire:failed', {
      lockKey,
      error: err.message,
    });
    // Degrade gracefully: allow the job to run rather than silently skip.
    return true;
  }
}

/**
 * Release the lock, but only if we still own it (compare-and-delete via Lua).
 */
const RELEASE_SCRIPT = `
if redis.call("get", KEYS[1]) == ARGV[1] then
  return redis.call("del", KEYS[1])
else
  return 0
end
`;

async function releaseLock(lockKey, token) {
  const redis = getRedis();
  if (!redis) return;
  try {
    await redis.eval(RELEASE_SCRIPT, 1, lockKey, token);
  } catch (err) {
    logger.warn('scheduler:lock:release:failed', {
      lockKey,
      error: err.message,
    });
  }
}

// ── Job registry ──────────────────────────────────────────────────────────────

/**
 * @typedef {Object} JobDefinition
 * @property {string}            name        Unique job identifier.
 * @property {string}            schedule    Cron expression (5 or 6 fields).
 * @property {function():Promise} handler     Async function to run.
 * @property {number}            [lockTtlMs] Lock TTL in ms. Defaults to SCHEDULER_LOCK_TTL env var or 55 000.
 * @property {boolean}           [enabled]   Set to false to skip this job. Default true.
 */

/**
 * @type {Map<string, {definition: JobDefinition, task: cron.ScheduledTask|null}>}
 */
const _jobs = new Map();
let _started = false;

const DEFAULT_LOCK_TTL_MS = parseInt(
  process.env.SCHEDULER_LOCK_TTL || '55000',
  10
);

/**
 * Register a job. Must be called before `startScheduler()`, or the job will
 * start immediately if the scheduler is already running.
 *
 * @param {JobDefinition} definition
 */
export function registerJob(definition) {
  const {
    name,
    schedule,
    handler,
    lockTtlMs = DEFAULT_LOCK_TTL_MS,
    enabled = true,
  } = definition;

  if (!name || typeof name !== 'string')
    throw new TypeError('scheduler: job name must be a non-empty string');
  if (!schedule || typeof schedule !== 'string')
    throw new TypeError(
      `scheduler[${name}]: schedule must be a cron expression`
    );
  if (typeof handler !== 'function')
    throw new TypeError(`scheduler[${name}]: handler must be a function`);
  if (!cron.validate(schedule))
    throw new Error(
      `scheduler[${name}]: invalid cron expression "${schedule}"`
    );

  if (_jobs.has(name)) {
    logger.warn(`scheduler: job "${name}" already registered — overwriting`);
    const existing = _jobs.get(name);
    if (existing.task) existing.task.stop();
  }

  _jobs.set(name, {
    definition: { name, schedule, handler, lockTtlMs, enabled },
    task: null,
  });

  // If the scheduler is already running, schedule this job immediately.
  if (_started && enabled) {
    _scheduleJob(name);
  }
}

/**
 * Deregister a job by name and stop its underlying cron task if running.
 */
export function deregisterJob(name) {
  const entry = _jobs.get(name);
  if (!entry) return;
  if (entry.task) entry.task.stop();
  _jobs.delete(name);
}

/**
 * Returns a snapshot of all registered job names and their enabled state.
 */
export function listJobs() {
  return Array.from(_jobs.entries()).map(([name, { definition, task }]) => ({
    name,
    schedule: definition.schedule,
    enabled: definition.enabled,
    running: task !== null,
  }));
}

// ── Internal scheduling ───────────────────────────────────────────────────────

function _scheduleJob(name) {
  const entry = _jobs.get(name);
  if (!entry) return;

  const { definition } = entry;
  if (!definition.enabled) return;

  const task = cron.schedule(definition.schedule, () => _runJob(definition));

  entry.task = task;
  logger.info('scheduler:job:scheduled', {
    name,
    schedule: definition.schedule,
  });
}

async function _runJob(definition) {
  const { name, handler, lockTtlMs } = definition;
  const lockKey = `scheduler:lock:${name}`;
  const token = Math.random().toString(36).slice(2) + Date.now().toString(36);

  const acquired = await acquireLock(lockKey, token, lockTtlMs);
  if (!acquired) {
    logger.debug('scheduler:job:skipped (lock held by another replica)', {
      name,
    });
    return;
  }

  const start = Date.now();
  logger.info('scheduler:job:start', { name });

  try {
    await handler();
    const durationMs = Date.now() - start;
    logger.info('scheduler:job:success', { name, durationMs });
  } catch (err) {
    const durationMs = Date.now() - start;
    logger.error('scheduler:job:error', {
      name,
      durationMs,
      error: err.message,
      stack: err.stack,
    });
  } finally {
    await releaseLock(lockKey, token);
  }
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/**
 * Start all registered, enabled jobs.
 * Safe to call multiple times — subsequent calls are no-ops.
 */
export function startScheduler() {
  if (process.env.SCHEDULER_ENABLED === 'false') {
    logger.info('scheduler: disabled via SCHEDULER_ENABLED=false');
    return;
  }

  if (_started) {
    logger.info('scheduler: already started');
    return;
  }

  _started = true;
  logger.info('scheduler: starting', { jobCount: _jobs.size });

  for (const [name, entry] of _jobs.entries()) {
    if (entry.definition.enabled && !entry.task) {
      _scheduleJob(name);
    }
  }
}

/**
 * Stop all running cron tasks and release the Redis client.
 * The scheduler can be restarted by calling `startScheduler()` again.
 */
export async function stopScheduler() {
  _started = false;
  for (const [name, entry] of _jobs.entries()) {
    if (entry.task) {
      entry.task.stop();
      entry.task = null;
      logger.info('scheduler:job:stopped', { name });
    }
  }

  if (_redis) {
    try {
      await _redis.quit();
    } catch {
      // ignore
    }
    _redis = null;
    _redisChecked = false;
  }

  logger.info('scheduler: stopped');
}

export default {
  registerJob,
  deregisterJob,
  listJobs,
  startScheduler,
  stopScheduler,
};
