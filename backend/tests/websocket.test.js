import { EventEmitter } from 'events';
import { jest } from '@jest/globals';

// Capture the connection handler set by setupWebsocketServer
let connectionHandler = null;
const wssHandlers = {};

jest.mock('ws', () => ({
  __esModule: true,
  WebSocketServer: jest.fn(() => ({
    on: jest.fn((event, handler) => {
      wssHandlers[event] = handler;
      if (event === 'connection') connectionHandler = handler;
    }),
  })),
}));

jest.mock('../src/utils/tracing.js', () => ({
  __esModule: true,
  createSpan: jest.fn(() => ({ end: jest.fn() })),
  setSpanAttributes: jest.fn(),
  addSpanEvent: jest.fn(),
  getTraceId: jest.fn(),
}));

jest.mock('../src/utils/alerting.js', () => ({
  __esModule: true,
  alertManager: { alert: jest.fn() },
}));

jest.mock('../src/config/index.js', () => ({
  __esModule: true,
  default: {
    rateLimit: {
      global: { max: 100, windowMs: 60000 },
      compile: { max: 10, windowMs: 60000 },
    },
    tracing: { serviceName: 'test', serviceVersion: '0.0.0', enabled: false },
    compile: { timeoutMs: 30000 },
  },
}));

jest.mock('../src/services/invokeService.js', () => {
  const { EventEmitter } = require('events');
  return { __esModule: true, invokeProgressBus: new EventEmitter() };
});
jest.mock('../src/services/deployService.js', () => {
  const { EventEmitter } = require('events');
  return { __esModule: true, deployProgressBus: new EventEmitter() };
});
jest.mock('../src/services/compileService.js', () => {
  const { EventEmitter } = require('events');
  return { __esModule: true, compileProgressBus: new EventEmitter() };
});
jest.mock('../src/services/oracleProofQueueService.js', () => {
  const { EventEmitter } = require('events');
  return { __esModule: true, default: new EventEmitter() };
});
jest.mock('../src/services/oracle/oracleEvents.js', () => ({
  __esModule: true,
  sharedOracleEventBus: { on: jest.fn() },
}));
jest.mock('../src/services/contractEventParser.js', () => ({
  __esModule: true,
  registerHandler: jest.fn(),
  dispatchEvent: jest.fn(),
}));
jest.mock('../src/services/redisService.js', () => ({
  __esModule: true,
  default: { isFallbackMode: true, client: null },
}));

import {
  setupWebsocketServer,
  closeWebsocketServer,
  broadcast,
  broadcastTreasuryEvent,
  broadcastContractEvent,
  broadcastCompilationProgress,
  broadcastTerminalLog,
  broadcastCluster,
  REDIS_WS_CHANNELS,
} from '../src/websocket.js';

function makeSocket(readyState = 1) {
  const handlers = {};
  return {
    readyState,
    OPEN: 1,
    send: jest.fn(),
    close: jest.fn(),
    on: jest.fn((event, fn) => {
      handlers[event] = fn;
    }),
    _handlers: handlers,
  };
}

function makeRequest(url = '/ws', headers = {}) {
  return { url, headers };
}

beforeAll(() => {
  setupWebsocketServer({ on: jest.fn() });
});

afterAll(() => {
  closeWebsocketServer();
});

describe('WebSocket server', () => {
  it('sends connected message on new connection', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest());
    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"connected"')
    );
  });

  it('closes socket with 1008 when URL is malformed', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest('http://'));
    expect(socket.close).toHaveBeenCalledWith(1008, 'Bad Request');
    expect(socket.send).not.toHaveBeenCalled();
  });

  it('closes socket with 1008 when auth token is wrong', () => {
    process.env.WS_AUTH_TOKEN = 'secret';
    const socket = makeSocket();
    connectionHandler(
      socket,
      makeRequest('/ws', { authorization: 'Bearer wrong' })
    );
    expect(socket.close).toHaveBeenCalledWith(1008, 'Unauthorized');
    delete process.env.WS_AUTH_TOKEN;
  });

  it('accepts connection when token matches via query param', () => {
    process.env.WS_AUTH_TOKEN = 'secret';
    const socket = makeSocket();
    connectionHandler(socket, makeRequest('/ws?token=secret'));
    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"connected"')
    );
    delete process.env.WS_AUTH_TOKEN;
  });

  it('removes client on socket close so broadcast skips it', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest());
    socket.send.mockClear();

    socket._handlers['close']();
    broadcast({ type: 'test' });

    expect(socket.send).not.toHaveBeenCalled();
  });

  it('removes client on socket error without throwing', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest());
    socket.send.mockClear();

    expect(() => socket._handlers['error'](new Error('reset'))).not.toThrow();
    broadcast({ type: 'test' });

    expect(socket.send).not.toHaveBeenCalled();
  });

  it('does not send to non-OPEN sockets', () => {
    const socket = makeSocket(3); // CLOSING
    connectionHandler(socket, makeRequest());
    expect(socket.send).not.toHaveBeenCalled();
  });

  it('broadcast sends to all open clients', () => {
    const s1 = makeSocket();
    const s2 = makeSocket();
    connectionHandler(s1, makeRequest());
    connectionHandler(s2, makeRequest());
    s1.send.mockClear();
    s2.send.mockClear();

    broadcast({ type: 'ping' });

    expect(s1.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"ping"')
    );
    expect(s2.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"ping"')
    );
  });

  it('broadcast handles non-serializable payload without throwing', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest());
    socket.send.mockClear();

    const circular = {};
    circular.self = circular;

    expect(() => broadcast(circular)).not.toThrow();
    expect(socket.send).not.toHaveBeenCalled();
  });

  it('broadcastTreasuryEvent sends treasury-event type to clients', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest());
    socket.send.mockClear();

    broadcastTreasuryEvent({ proposalId: 1, action: 'vote' });

    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"treasury-event"')
    );
  });

  it('broadcastContractEvent sends contract-event type across cluster', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest());
    socket.send.mockClear();

    broadcastContractEvent({
      contractId: 'CC123',
      topics: ['transfer'],
      value: 100,
    });

    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"contract-event"')
    );
    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('"contractId":"CC123"')
    );
  });

  it('broadcastCompilationProgress sends compile-progress type across cluster', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest());
    socket.send.mockClear();

    broadcastCompilationProgress({ step: 'building', progress: 50 });

    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"compile-progress"')
    );
    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('"step":"building"')
    );
  });

  it('broadcastTerminalLog sends terminal-log type across cluster', () => {
    const socket = makeSocket();
    connectionHandler(socket, makeRequest());
    socket.send.mockClear();

    broadcastTerminalLog({
      sessionId: 'term-1',
      data: 'Compiling contract...',
    });

    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"terminal-log"')
    );
    expect(socket.send).toHaveBeenCalledWith(
      expect.stringContaining('Compiling contract...')
    );
  });

  it('exposes defined Redis cluster channels', () => {
    expect(REDIS_WS_CHANNELS.BROADCAST).toBe('ws:broadcast');
    expect(REDIS_WS_CHANNELS.CONTRACT_EVENTS).toBe(
      'ws:channel:contract-events'
    );
    expect(REDIS_WS_CHANNELS.COMPILATION_PROGRESS).toBe(
      'ws:channel:compilation-progress'
    );
    expect(REDIS_WS_CHANNELS.TERMINAL_LOGS).toBe('ws:channel:terminal-logs');
  });
});
