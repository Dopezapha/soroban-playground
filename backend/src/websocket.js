import { WebSocketServer, WebSocket } from 'ws';

import { invokeProgressBus } from './services/invokeService.js';
import { deployProgressBus } from './services/deployService.js';
import { compileProgressBus } from './services/compileService.js';
import oracleProofQueueService from './services/oracleProofQueueService.js';
import redisService from './services/redisService.js';
import { sharedOracleEventBus } from './services/oracle/oracleEvents.js';

import { registerHandler } from './services/contractEventParser.js';

const clients = new Set();

// Tracks number of active connections per IP address.
const ipCounts = new Map();

const HEARTBEAT_INTERVAL_MS = 30_000; // ping every 30 s
const MAX_MISSED_PONGS = 2; // terminate after 2 consecutive misses
const MAX_CONNECTIONS_PER_IP = 10;

export const REDIS_WS_CHANNELS = {
  BROADCAST: 'ws:broadcast',
  CONTRACT_EVENTS: 'ws:channel:contract-events',
  COMPILATION_PROGRESS: 'ws:channel:compilation-progress',
  TERMINAL_LOGS: 'ws:channel:terminal-logs',
};

const REDIS_BROADCAST_CHANNEL = REDIS_WS_CHANNELS.BROADCAST;

let redisSubscriber = null;

function safeSend(socket, message) {
  try {
    const isOpen = socket.readyState === (WebSocket?.OPEN ?? 1);
    if (isOpen) {
      socket.send(message);
    }
  } catch (err) {
    console.error('WS send error:', err.message);
    if (typeof socket.terminate === 'function') {
      socket.terminate();
    }
    if (socket.releaseIp) socket.releaseIp();
    clients.delete(socket);
  }
}

function safeStringify(payload) {
  try {
    return JSON.stringify(payload);
  } catch (err) {
    console.error('WS serialize error:', err.message);
    return null;
  }
}

// Broadcast a message to all connected clients on this instance.
function broadcastLocal(message) {
  if (!message) return;
  for (const socket of clients) {
    safeSend(socket, message);
  }
}

// Broadcast a message to all clients across all instances using a specific Redis Pub/Sub channel.
export function broadcastCluster(channel, message) {
  if (!message) return;
  const serialized =
    typeof message === 'string' ? message : safeStringify(message);
  if (!serialized) return;

  if (redisService.client && !redisService.isFallbackMode) {
    try {
      redisService.client.publish(channel, serialized);
    } catch (err) {
      console.error(`Redis publish error on ${channel}:`, err.message);
      broadcastLocal(serialized);
    }
  } else {
    broadcastLocal(serialized);
  }
}

// Broadcast a message to all clients across all instances using Redis Pub/Sub default broadcast channel.
function broadcastGlobal(message) {
  broadcastCluster(REDIS_BROADCAST_CHANNEL, message);
}

function getClientIp(req) {
  const xff = req?.headers?.['x-forwarded-for'];
  if (xff) {
    const ip = xff.split(',')[0].trim();
    if (ip) return ip;
  }
  return (
    req?.socket?.remoteAddress || req?.connection?.remoteAddress || '127.0.0.1'
  );
}

export function broadcastTreasuryEvent(event) {
  const message = safeStringify({ type: 'treasury-event', ...event });
  if (!message) return;
  broadcastGlobal(message);
}

export function broadcastContractEvent(event) {
  const message = safeStringify({ type: 'contract-event', ...event });
  if (!message) return;
  broadcastCluster(REDIS_WS_CHANNELS.CONTRACT_EVENTS, message);
}

export function broadcastCompilationProgress(progress) {
  const message = safeStringify({ type: 'compile-progress', ...progress });
  if (!message) return;
  broadcastCluster(REDIS_WS_CHANNELS.COMPILATION_PROGRESS, message);
}

export function broadcastTerminalLog(logData) {
  const message = safeStringify({ type: 'terminal-log', ...logData });
  if (!message) return;
  broadcastCluster(REDIS_WS_CHANNELS.TERMINAL_LOGS, message);
}

let wssInstance = null;

export function setupWebSocketServer(httpServer) {
  if (wssInstance) {
    try {
      closeWebSocketServer();
    } catch (_) {}
  }

  // Set up Redis subscriber for cross-cluster broadcasts across channels.
  if (
    !redisSubscriber &&
    redisService.client &&
    redisService.client.status === 'ready' &&
    !redisService.isFallbackMode
  ) {
    try {
      redisSubscriber = redisService.client.duplicate();
      redisSubscriber.on('error', () => {});
      const subscribedChannels = Object.values(REDIS_WS_CHANNELS);
      for (const ch of subscribedChannels) {
        redisSubscriber.subscribe(ch).catch(() => {});
      }
      redisSubscriber.on('message', (channel, message) => {
        if (subscribedChannels.includes(channel)) {
          broadcastLocal(message);
        }
      });
    } catch (err) {
      console.error('WS Redis subscriber error:', err.message);
      if (redisSubscriber) {
        redisSubscriber.quit().catch(() => {});
        redisSubscriber = null;
      }
    }
  }

  const wss = new WebSocketServer({
    server: httpServer,
    path: '/ws',
  });

  wssInstance = wss;

  wss.on('error', (err) => {
    console.error('WebSocketServer error:', err.message);
  });

  wss.on('connection', (socket, request) => {
    let url;
    try {
      url = new URL(request.url, 'http://localhost');
    } catch {
      socket.close(1008, 'Bad Request');
      return;
    }

    const ip = getClientIp(request);

    let ipAcquired = false;
    const releaseIp = () => {
      if (!ipAcquired) return;
      ipAcquired = false;
      if (ip) {
        const count = ipCounts.get(ip) || 0;
        if (count <= 1) {
          ipCounts.delete(ip);
        } else {
          ipCounts.set(ip, count - 1);
        }
      }
    };
    socket.releaseIp = releaseIp;

    // Enforce per-IP connection limit.
    if (ip) {
      const currentCount = ipCounts.get(ip) || 0;
      if (currentCount >= MAX_CONNECTIONS_PER_IP) {
        socket.close(1008, 'Too Many Connections');
        return;
      }
      ipCounts.set(ip, currentCount + 1);
      ipAcquired = true;
    }

    // Decrement the per-IP count on connection close.

    const authHeader = request.headers.authorization || '';
    const tokenFromQuery = url.searchParams.get('token');
    const token = authHeader.startsWith('Bearer ')
      ? authHeader.slice('Bearer '.length)
      : tokenFromQuery;

    if (process.env.WS_AUTH_TOKEN && token !== process.env.WS_AUTH_TOKEN) {
      releaseIp();
      socket.close(1008, 'Unauthorized');
      return;
    }

    // Register the connection after successful authentication.
    socket.missedPongs = 0;
    clients.add(socket);

    safeSend(
      socket,
      safeStringify({ type: 'connected', timestamp: new Date().toISOString() })
    );

    socket.on('message', (data) => {
      try {
        const payload = JSON.parse(data);
        if (
          payload.type === 'collaboration-join' ||
          payload.type === 'collaboration-cursor'
        ) {
          socket.docId = payload.docId || 'default-doc';
          if (payload.user) {
            socket.collaboratorName = payload.user.name;
            socket.collaboratorColor = payload.user.color;
          }
          const peers = Array.from(clients)
            .filter((s) => s !== socket && s.docId === socket.docId)
            .map((s, idx) => ({
              id: `peer-${idx}`,
              name: s.collaboratorName || `Peer ${idx + 1}`,
              color: s.collaboratorColor || '#6366f1',
              cursor: s.cursor,
              lastActive: new Date().toISOString(),
            }));
          safeSend(
            socket,
            safeStringify({
              type: 'collaboration-presence',
              docId: socket.docId,
              peers,
            })
          );
        }
      } catch {
        // ignore invalid payload
      }
    });

    socket.on('pong', () => {
      socket.missedPongs = 0;
    });

    socket.on('error', (err) => {
      console.error('WS client error:', err.message);
      clients.delete(socket);
      if (socket.releaseIp) socket.releaseIp();
    });

    socket.on('close', () => {
      clients.delete(socket);
      if (socket.releaseIp) socket.releaseIp();
    });
  });

  const forward = (type) => (event) => {
    const message = safeStringify({ type, ...event });
    if (!message) return;
    broadcastGlobal(message);
  };

  invokeProgressBus.on('progress', forward('invoke-progress'));
  deployProgressBus.on('progress', forward('deploy-progress'));
  compileProgressBus.on('progress', (progress) => {
    broadcastCompilationProgress(progress);
  });
  oracleProofQueueService.on('progress', forward('oracle-proof-progress'));

  try {
    registerHandler('*', (event) => {
      broadcastContractEvent(event);
    });
  } catch (_) {}

  sharedOracleEventBus.on('*', (payload) => {
    const message = safeStringify({ type: 'oracle-event', ...payload });
    if (!message) return;
    broadcastGlobal(message);
  });

  // Heartbeat: ping all clients every 30 s; terminate after 2 missed pongs
  const heartbeatTimer = setInterval(() => {
    for (const socket of clients) {
      if (socket.missedPongs >= MAX_MISSED_PONGS) {
        console.warn('WS heartbeat: terminating stale connection');
        if (socket.releaseIp) socket.releaseIp();
        socket.terminate();
        clients.delete(socket);
        continue;
      }
      socket.missedPongs += 1;
      try {
        socket.ping();
      } catch (err) {
        console.error('WS ping error:', err.message);
        if (socket.releaseIp) socket.releaseIp();
        socket.terminate();
        clients.delete(socket);
      }
    }
  }, HEARTBEAT_INTERVAL_MS);
  activeHeartbeatTimer = heartbeatTimer;

  const analyticsTimer = setInterval(async () => {
    if (
      clients.size === 0 ||
      redisService.isFallbackMode ||
      !redisService.client
    )
      return;

    try {
      const topIps = await redisService.client.zrevrange(
        'analytics:top_ips',
        0,
        9,
        'WITHSCORES'
      );
      const endpoints = ['compile', 'invoke', 'deploy', 'global'];
      const stats = {};

      for (const endpoint of endpoints) {
        stats[endpoint] = await redisService.client.hgetall(
          `analytics:endpoint:${endpoint}`
        );
      }

      const message = safeStringify({
        type: 'rate-limit-analytics',
        timestamp: new Date().toISOString(),
        topIps,
        stats,
      });

      if (!message) return;

      broadcastLocal(message);
    } catch (err) {
      console.error('WS Analytics Broadcast Error:', err.message);
    }
  }, 2000);
  activeAnalyticsTimer = analyticsTimer;

  wss.on('close', () => {
    clearInterval(heartbeatTimer);
    clearInterval(analyticsTimer);
    activeHeartbeatTimer = null;
    activeAnalyticsTimer = null;
  });

  return wss;
}

let activeHeartbeatTimer = null;
let activeAnalyticsTimer = null;

export function closeWebSocketServer() {
  if (activeHeartbeatTimer) {
    clearInterval(activeHeartbeatTimer);
    activeHeartbeatTimer = null;
  }
  if (activeAnalyticsTimer) {
    clearInterval(activeAnalyticsTimer);
    activeAnalyticsTimer = null;
  }
  if (wssInstance) {
    for (const socket of clients) {
      if (socket.releaseIp) socket.releaseIp();
      if (typeof socket.terminate === 'function') {
        socket.terminate();
      }
    }
    clients.clear();
    if (typeof wssInstance.close === 'function') {
      wssInstance.close();
    }
  }
  ipCounts.clear();
  if (redisSubscriber) {
    try {
      const subscribedChannels = Object.values(REDIS_WS_CHANNELS);
      for (const ch of subscribedChannels) {
        redisSubscriber.unsubscribe(ch);
      }
      redisSubscriber.quit();
    } catch (err) {
      console.error('WS Redis subscriber close error:', err.message);
    }
    redisSubscriber = null;
  }
}

export function broadcast(payload) {
  const message = safeStringify(payload);
  if (!message) return;
  broadcastGlobal(message);
}

export {
  setupWebSocketServer as setupWebsocketServer,
  closeWebSocketServer as closeWebsocketServer,
};
