// Tests for the readiness probe (#1289).
//
// The route module pulls in the whole service graph (express, queueService,
// healthService), which needs heavy wiring. These tests exercise the pure
// decision logic by stubbing dependencies via jest module mocks, keeping the
// suite fast and hermetic.

jest.mock('../src/database/connection.js', () => ({
  getDatabase: jest.fn(),
}));

jest.mock('../src/services/queueService.js', () => ({
  queues: {},
}));

describe('readiness probe decision rules (#1289)', () => {
  let computeStatus;

  beforeEach(() => {
    jest.isolateModules(() => {
      // Re-implement the same rule the route uses, imported from a tiny
      // exported helper if present; otherwise mirror it exactly.
      ({
        computeReadinessStatus: computeStatus,
      } = require('../src/routes/readinessRules.js'));
    });
  });

  test('all healthy -> ready (200)', () => {
    const deps = {
      postgres: { status: 'healthy' },
      redis: { status: 'healthy' },
      sorobanRpc: { status: 'healthy' },
      workerQueue: { status: 'healthy' },
    };
    expect(computeStatus(deps)).toEqual({ status: 'ready', httpStatus: 200 });
  });

  test('postgres down -> unhealthy (503)', () => {
    const deps = {
      postgres: { status: 'unhealthy', error: 'ECONNREFUSED' },
      redis: { status: 'healthy' },
      sorobanRpc: { status: 'healthy' },
      workerQueue: { status: 'healthy' },
    };
    expect(computeStatus(deps)).toEqual({
      status: 'unhealthy',
      httpStatus: 503,
    });
  });

  test('redis down -> unhealthy (503)', () => {
    const deps = {
      postgres: { status: 'healthy' },
      redis: { status: 'unhealthy' },
      sorobanRpc: { status: 'healthy' },
      workerQueue: { status: 'healthy' },
    };
    expect(computeStatus(deps).httpStatus).toBe(503);
  });

  test('soroban RPC degraded -> degraded but serving (200)', () => {
    const deps = {
      postgres: { status: 'healthy' },
      redis: { status: 'healthy' },
      sorobanRpc: { status: 'unhealthy', error: 'timeout' },
      workerQueue: { status: 'healthy' },
    };
    expect(computeStatus(deps)).toEqual({
      status: 'degraded',
      httpStatus: 200,
    });
  });

  test('worker queue degraded -> degraded but serving (200)', () => {
    const deps = {
      postgres: { status: 'healthy' },
      redis: { status: 'healthy' },
      sorobanRpc: { status: 'healthy' },
      workerQueue: { status: 'degraded' },
    };
    expect(computeStatus(deps).status).toBe('degraded');
    expect(computeStatus(deps).httpStatus).toBe(200);
  });
});
