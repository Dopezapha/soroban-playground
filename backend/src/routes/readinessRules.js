// Pure decision rules for the readiness probe (#1289).
// Kept dependency-free so tests run hermetically.

/**
 * @param {Object} deps - per-dependency results with .status
 * @param {{status: string}} deps.postgres
 * @param {{status: string}} deps.redis
 * @param {{status: string}} [deps.sorobanRpc]
 * @param {{status: string}} [deps.workerQueue]
 * @returns {{status: 'ready'|'degraded'|'unhealthy', httpStatus: number}}
 */
export function computeReadinessStatus(deps) {
  const criticalDown = ['postgres', 'redis'].some(
    (k) => deps[k]?.status !== 'healthy'
  );
  const degraded = ['sorobanRpc', 'workerQueue'].some(
    (k) => deps[k]?.status === 'unhealthy' || deps[k]?.status === 'degraded'
  );

  if (criticalDown) return { status: 'unhealthy', httpStatus: 503 };
  if (degraded) return { status: 'degraded', httpStatus: 200 };
  return { status: 'ready', httpStatus: 200 };
}
