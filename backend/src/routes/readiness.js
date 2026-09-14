// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

// Readiness probe with deep dependency validation (#1289).
//
// Verifies PostgreSQL, Redis, Soroban RPC and the BullMQ worker queue are
// actually reachable before traffic is routed to this instance. Unlike
// liveness (/health/live), a failing readiness check removes the pod from
// the load balancer until dependencies recover.

import express from 'express';
import { asyncHandler } from '../middleware/errorHandler.js';
import healthService from '../services/healthService.js';
import { getDatabase } from '../database/connection.js';
import { queues } from '../services/queueService.js';

const router = express.Router();

const READY_TIMEOUT_MS = 3000;

function withTimeout(promise, label) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`${label} timed out after ${READY_TIMEOUT_MS}ms`)),
      READY_TIMEOUT_MS
    );
  });
  return Promise.race([
    Promise.resolve(promise).finally(() => clearTimeout(timer)),
    timeout,
  ]);
}

async function checkPostgres() {
  try {
    const db = getDatabase();
    if (!db || typeof db.raw !== 'function') {
      // sqlite/knex-style handle — try a trivial select
      await withTimeout(db.select(1).limit(1) ?? Promise.resolve(), 'postgres');
    } else {
      await withTimeout(db.raw('SELECT 1'), 'postgres');
    }
    return { status: 'healthy' };
  } catch (error) {
    return { status: 'unhealthy', error: error.message };
  }
}

async function checkWorkerQueue() {
  try {
    const names = Object.keys(queues || {});
    if (names.length === 0) {
      return { status: 'degraded', detail: 'no queues initialized' };
    }
    const counts = await withTimeout(
      Promise.all(
        names.map(async (name) => {
          const q = queues[name];
          const jobCounts =
            typeof q.getJobCounts === 'function'
              ? await q.getJobCounts(['active', 'failed'])
              : {};
          return {
            queue: name,
            active: jobCounts.active ?? 0,
            failed: jobCounts.failed ?? 0,
          };
        })
      ),
      'worker-queue'
    );
    return { status: 'healthy', queues: counts };
  } catch (error) {
    return { status: 'unhealthy', error: error.message };
  }
}

export const readinessHandler = asyncHandler(async (_req, res) => {
  const startedAt = Date.now();

  const [postgres, redis, sorobanRpc, workerQueue] = await Promise.allSettled([
    checkPostgres(),
    healthService.dependencyCheckers.redis
      ? healthService.dependencyCheckers.redis()
      : Promise.resolve({ status: 'unknown' }),
    healthService.dependencyCheckers.sorobanRpc
      ? healthService.dependencyCheckers.sorobanRpc()
      : Promise.resolve({ status: 'unknown' }),
    checkWorkerQueue(),
  ]);

  const value = (r, fallback) =>
    r.status === 'fulfilled' ? r.value : fallback;
  const dependencies = {
    postgres: value(postgres, { status: 'unhealthy', error: 'check crashed' }),
    redis: value(redis, { status: 'unhealthy', error: 'check crashed' }),
    sorobanRpc: value(sorobanRpc, {
      status: 'unhealthy',
      error: 'check crashed',
    }),
    workerQueue: value(workerQueue, {
      status: 'unhealthy',
      error: 'check crashed',
    }),
  };

  const criticalDown = ['postgres', 'redis'].some(
    (k) => dependencies[k].status !== 'healthy'
  );
  const degraded = ['sorobanRpc', 'workerQueue'].some(
    (k) => dependencies[k].status === 'unhealthy'
  );

  const status = criticalDown ? 'unhealthy' : degraded ? 'degraded' : 'ready';
  const httpStatus =
    status === 'ready' ? 200 : status === 'degraded' ? 200 : 503;

  return res.status(httpStatus).json({
    success: status !== 'unhealthy',
    data: {
      status,
      probe: 'readiness',
      durationMs: Date.now() - startedAt,
      timestamp: new Date().toISOString(),
      dependencies,
    },
  });
});

router.get('/ready', readinessHandler);

export default router;
