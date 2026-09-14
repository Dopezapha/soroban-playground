// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

/**
 * Winston Structured JSON Logger with Correlation IDs & PII Masking
 *
 * - Every log line carries a `traceId` injected via AsyncLocalStorage so that
 *   all log entries for a single request share the same identifier without
 *   passing context through every call frame.
 * - PII masking strips private keys (56-char Stellar secret seeds starting with
 *   'S'), JWT tokens, Bearer tokens, passwords, and generic `secret` fields
 *   before the record hits any transport.
 * - Falls back gracefully to a console-based implementation when `winston` is
 *   not available (e.g. during test runs that mock the module graph).
 *
 * Usage:
 *   import logger, { runWithTraceId, generateTraceId } from './logger.js';
 *
 *   // In Express middleware:
 *   app.use((req, res, next) => {
 *     const traceId = req.headers['x-trace-id'] || generateTraceId();
 *     runWithTraceId(traceId, next);
 *   });
 *
 *   // Anywhere in the request lifecycle:
 *   logger.info('user_action', { userId: '...' });
 */

import { AsyncLocalStorage } from 'async_hooks';

// ── Trace-ID store ────────────────────────────────────────────────────────────

const traceStore = new AsyncLocalStorage();

/**
 * Generates a short random trace identifier (16 hex chars).
 */
export function generateTraceId() {
  return (
    Math.random().toString(16).slice(2).padEnd(8, '0') +
    Math.random().toString(16).slice(2).padEnd(8, '0')
  ).slice(0, 16);
}

/**
 * Runs `fn` inside an async context that carries the given `traceId`.
 * All logger calls made within `fn` (or anything it awaits) will automatically
 * include this traceId.
 */
export function runWithTraceId(traceId, fn) {
  return traceStore.run({ traceId }, fn);
}

/**
 * Returns the traceId for the current async context, or 'none'.
 */
export function getCurrentTraceId() {
  const store = traceStore.getStore();
  return (store && store.traceId) || 'none';
}

// ── PII masking ───────────────────────────────────────────────────────────────

/** Keys whose values should always be masked. */
const SENSITIVE_KEYS = new Set([
  'password',
  'secret',
  'secretKey',
  'secret_key',
  'privateKey',
  'private_key',
  'accessToken',
  'access_token',
  'refreshToken',
  'refresh_token',
  'apiKey',
  'api_key',
  'authorization',
  'Authorization',
  'token',
  'seed',
  'mnemonic',
]);

/** Regex patterns that identify sensitive string values regardless of key. */
const SENSITIVE_VALUE_PATTERNS = [
  // Stellar secret seeds: S + 55 uppercase base32 chars
  /\bS[A-Z2-7]{55}\b/g,
  // JWT bearer tokens
  /\bBearer\s+[A-Za-z0-9._-]{20,}\b/gi,
  // Generic JWT (three base64url segments)
  /\bey[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b/g,
];

const MASK = '[REDACTED]';

/**
 * Deep-clones `obj` and replaces sensitive field values with `[REDACTED]`.
 * Handles nested objects and arrays. Circular references are broken (replaced
 * with a placeholder string).
 */
export function maskPii(obj, _seen = new WeakSet()) {
  if (obj === null || typeof obj !== 'object') {
    return maskString(obj);
  }

  if (_seen.has(obj)) return '[Circular]';
  _seen.add(obj);

  if (Array.isArray(obj)) {
    return obj.map((item) => maskPii(item, _seen));
  }

  const masked = {};
  for (const [key, value] of Object.entries(obj)) {
    if (SENSITIVE_KEYS.has(key)) {
      masked[key] = MASK;
    } else if (typeof value === 'string') {
      masked[key] = maskString(value);
    } else if (typeof value === 'object' && value !== null) {
      masked[key] = maskPii(value, _seen);
    } else {
      masked[key] = value;
    }
  }
  return masked;
}

function maskString(value) {
  if (typeof value !== 'string') return value;
  let result = value;
  for (const pattern of SENSITIVE_VALUE_PATTERNS) {
    // Reset lastIndex because we reuse the same regex objects.
    pattern.lastIndex = 0;
    result = result.replace(pattern, MASK);
  }
  return result;
}

// ── Winston integration ───────────────────────────────────────────────────────

/**
 * Attempts to create a proper Winston logger. If winston is not installed or
 * fails to load, we fall back to a lightweight console-based implementation
 * that preserves the same interface.
 */
function buildWinstonLogger() {
  try {
    // Dynamic import lets us avoid a hard dependency at module evaluation time.
    // In environments where winston isn't installed the catch block activates.
    const winston = await_require('winston');

    const { createLogger, format, transports } = winston;
    const { combine, timestamp, errors, json, colorize, simple } = format;

    /**
     * Custom format that:
     * 1. Injects the current traceId from AsyncLocalStorage.
     * 2. Runs PII masking over the entire log info object.
     */
    const traceAndMask = format((info) => {
      info.traceId = getCurrentTraceId();
      // Mask the message string itself.
      if (typeof info.message === 'string') {
        info.message = maskString(info.message);
      }
      // Mask any additional metadata fields passed as a second argument.
      if (info.meta && typeof info.meta === 'object') {
        info.meta = maskPii(info.meta);
      }
      // Walk all other top-level keys.
      for (const key of Object.keys(info)) {
        if (
          key === 'level' ||
          key === 'message' ||
          key === 'traceId' ||
          key === 'timestamp'
        )
          continue;
        if (SENSITIVE_KEYS.has(key)) {
          info[key] = MASK;
        } else if (typeof info[key] === 'object' && info[key] !== null) {
          info[key] = maskPii(info[key]);
        } else if (typeof info[key] === 'string') {
          info[key] = maskString(info[key]);
        }
      }
      return info;
    });

    const isDev =
      process.env.NODE_ENV === 'development' || process.env.LOG_PRETTY === '1';

    const transportList = [
      new transports.Console({
        format: isDev
          ? combine(colorize(), simple())
          : combine(
              timestamp(),
              errors({ stack: true }),
              traceAndMask(),
              json()
            ),
      }),
    ];

    return createLogger({
      level: process.env.LOG_LEVEL || 'info',
      format: combine(
        timestamp(),
        errors({ stack: true }),
        traceAndMask(),
        json()
      ),
      transports: transportList,
      // Do not exit on handled exceptions.
      exitOnError: false,
    });
  } catch {
    // winston not available; fall back to console implementation.
    return null;
  }
}

/**
 * Synchronous require wrapper used only inside the winston builder.
 * We use a dynamic `require` string to avoid static analysis errors in
 * ESM-strict environments — the actual import is gated behind a try/catch.
 */
function await_require(name) {
  // This will throw in pure ESM environments without a bundler.  The catch
  // block in buildWinstonLogger handles that gracefully.

  return require(name);
}

// ── Fallback console logger ───────────────────────────────────────────────────

/**
 * Minimal structured console logger used when winston is unavailable.
 * Produces JSON lines to stdout/stderr with the same fields winston would emit.
 */
function buildConsoleLogger() {
  const silent =
    process.env.NODE_ENV === 'test' && process.env.LOG_LEVEL !== 'debug';

  function write(level, message, meta) {
    if (silent) return;

    const entry = {
      level,
      message:
        typeof message === 'string' ? maskString(message) : String(message),
      traceId: getCurrentTraceId(),
      timestamp: new Date().toISOString(),
    };

    if (meta && typeof meta === 'object') {
      entry.meta = maskPii(meta);
    }

    const line = JSON.stringify(entry);
    if (level === 'error' || level === 'warn') {
      process.stderr.write(line + '\n');
    } else {
      process.stdout.write(line + '\n');
    }
  }

  return {
    error: (msg, meta) => write('error', msg, meta),
    warn: (msg, meta) => write('warn', msg, meta),
    info: (msg, meta) => write('info', msg, meta),
    http: (msg, meta) => write('http', msg, meta),
    verbose: (msg, meta) => write('verbose', msg, meta),
    debug: (msg, meta) => write('debug', msg, meta),
    silly: (msg, meta) => write('silly', msg, meta),
  };
}

// ── Logger instance ───────────────────────────────────────────────────────────

// Attempt winston; fall back to console logger.
let _logger;
try {
  _logger = buildWinstonLogger();
} catch {
  _logger = null;
}
if (!_logger) {
  _logger = buildConsoleLogger();
}

export { _logger as logger };
export default _logger;
