const WebSocket = require('ws');
const Redis = require('ioredis');

const HEARTBEAT_INTERVAL_MS = 30000;
const MAX_CONNECTIONS_PER_IP = 10;
const BROADCAST_CHANNEL = 'ws:broadcast';

class WebSocketService {
  constructor(server) {
    this.wss = new WebSocket.Server({ server });
    this.pub = new Redis(Process.env.REDIS_URL || 'redis://127.0.0.1:6379', {
      lazyConnect: true,
    });
    this.sub = new Redis(Process.env.REDIS_URL || 'redis://127.0.0.1:6379', {
      lazyConnect: true,
    });
    this.ipCounts = new Map();
    this.sub.on('message', (channel, message) => {
      if (channel === BROADCAST_CHANNEL) this.handleRedisMessage(message);
    });
    this.wss.on('connection', (ws, req) => this.handleConnection(ws, req));
    this.timer = setInterval(() => this.heartbeat(), HEARTBEAT_INTERVAL_MS);
    this.timer.unref?.();
    this.wss.on('close', () => this.close());
  }

  async init() {
    await this.pub.connect();
    await this.sub.connect();
    await this.sub.subscribe(BROADCAST_CHANNEL);
  }

  handleConnection(ws, req) {
    const ip = req.socket.remoteAddress?.replace(/^::ffff:/, '') || 'unknown';
    const count = this.ipCounts.get(ip) || 0;
    if (count >= MAX_CONNECTIONS_PER_IP) {
      ws.close(1008, 'Too many connections');
      return;
    }
    this.ipCounts.set(ip, count + 1);
    ws.accepted = true;
    ws.isAlive = true;
    ws.on('pong', () => {
      ws.isAlive = true;
    });
    ws.on('close', () => {
      if (!ws.accepted) return;
      const remaining = (this.ipCounts.get(ip) || 1) - 1;
      if (remaining <= 0) this.ipCounts.delete(ip);
      else this.ipCounts.set(ip, remaining);
    });
    ws.on('error', () => {});
  }

  heartbeat() {
    for (const ws of this.wss.clients) {
      if (!ws.isAlive) {
        ws.terminate();
        continue;
      }
      ws.isAlive = false;
      if (ws.readyState === WebSocket.OPEN) {
        ws.ping();
      }
    }
  }

  broadcast(data) {
    this.pub.publish(BROADCAST_CHANNEL, JSON.stringify({ data }));
  }

  handleRedisMessage(message) {
    let msg;
    try {
      msg = JSON.parse(message);
    } catch {
      return;
    }
    for (const client of this.wss.clients) {
      if (client.readyState === WebSocket.OPEN)
        client.send(JSON.stringify(msg.data));
    }
  }

  close() {
    clearInterval(this.timer);
    this.wss.clients.forEach((c) => c.terminate());
    this.wss.close();
    this.pub.disconnect();
    this.sub.disconnect();
  }
}

module.exports = WebSocketService;
