import { WebSocketServer, WebSocket } from 'ws';

import { invokeProgressBus } from './services/invokeService.js';
import { deployProgressBus } from './services/deployService.js';
import { compileProgressBus } from './services/compileService.js';
import oracleProofQueueService from './services/oracleProofQueueService.js';
import redisService from './services/redisService.js';
import { sharedOracleEventBus } from './services/oracle/oracleEvents.js';

const clients = new Set();

// Tracks number of active connections per IP address.
const ipCounts = new Map();

const HEARTBEAT_INTERVAL_MS = 30_000; // ping every 30 s
const MAX_MISSED_PONGS = 2; // terminate after 2 consecutive misses
const MAX_CONNECTIONS_PER_IP = 10;
const REDIS_BROADCAST_CHANNEL = 'ws:broadcast';

let redisSubscriber = null;

function safeSend(socket, message) {
  try {
    if (socket.readyState === WebSocket.OPEN) {
      socket.send(message);
    }
  } catch (err) {
    console.error('WS send error:', err.message);
    socket.terminate();
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

// Broadcast a message to all clients across all instances using Redis Pub/Sub.
function broadcastGlobal(message) {
  if (!message) return;
  if (redisService.client && !redisService.isFallbackMode) {
    try {
      redisService.client.publish(REDIS_BROADCAST_CHANNEL, message);
    } catch (err) {
      console.error('Redis publish error:', err.message);
      broadcastLocal(message);
    }
  } else {
    broadcastLocal(message);
  }
}

function getClientIp(req) {
  const xff = req.headers['x-forwarded-for'];
  if (xff) {
    const ip = xff.split(',')[0].trim();
    if (ip) return ip;
  }
  return req.socket.remoteAddress;
}

export function broadcastTreasuryEvent(event) {
  const message = safeStringify({ type: 'treasury-event', ...event });
  if (!message) return;
  broadcastGlobal(message);
}

let wssInstance = null;

export function setupWebSocketServer(httpServer) {
  if (wssInstance) {
    try {
      closeWebSocketServer();
    } catch (_) {}
  }

  // Set up Redis subscriber for cross-cluster broadcasts.
  if (!redisSubscriber && redisService.client && !redisService.isFallbackMode) {
    try {
      redisSubscriber = redisService.client.duplicate();
      redisSubscriber.subscribe(REDIS_BROADCAST_CHANNEL);
      redisSubscriber.on('message', (channel, message) => {
        if (channel === REDIS_BROADCAST_CHANNEL) {
          broadcastLocal(message);
        }
      });
    } catch (err) {
      console.error('WS Redis subscriber error:', err.message);
      if (redisSubscriber) {
        redisSubscriber.quit();
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
  compileProgressBus.on('progress', forward('compile-progress'));
  oracleProofQueueService.on('progress', forward('oracle-proof-progress'));

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

  wss.on('close', () => clearInterval(heartbeatTimer));

  // Broadcast analytics every 2 seconds
  setInterval(async () => {
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

  return wss;
}

export function closeWebSocketServer() {
  if (wssInstance) {
    for (const socket of clients) {
      if (socket.releaseIp) socket.releaseIp();
      socket.terminate();
    }
    clients.clear();
    wssInstance.close();
  }
  ipCounts.clear();
  if (redisSubscriber) {
    try {
      redisSubscriber.unsubscribe(REDIS_BROADCAST_CHANNEL);
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
