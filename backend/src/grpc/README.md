# gRPC Inter-Service Communication Layer

## Issue #1294 — Microservice Service Discovery & gRPC Inter-Service Communication

This document describes the gRPC communication layer added to `backend/src/grpc/`.

---

## Architecture

```
┌─────────────────────────────┐        gRPC (port 50051)        ┌──────────────────────────────┐
│   Express Backend (Node.js) │ ◄──────────────────────────────► │   Rust Indexer               │
│   backend/src/grpc/         │                                   │   indexer/src/               │
│   • server.js               │                                   │   • main.rs                  │
│   • client.js               │                                   │   • graphql/                 │
│   • serviceRegistry.js      │                                   │   • db/                      │
└─────────────────────────────┘                                   └──────────────────────────────┘
```

---

## Files

| File                    | Purpose                                                                                                                 |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `soroban_indexer.proto` | Protocol Buffer definitions for all messages and the `IndexerService`                                                   |
| `server.js`             | gRPC server — implements `IndexerService` handlers and exposes `grpcEventBus`                                           |
| `client.js`             | `GrpcClient` — promise-wrapped stub with exponential-backoff retry                                                      |
| `serviceRegistry.js`    | `ServiceRegistry` — health-polling, round-robin load balancing, TTL eviction                                            |
| `index.js`              | Re-exports: `startGrpcServer`, `shutdownGrpcServer`, `grpcEventBus`, `GrpcClient`, `ServiceRegistry`, `serviceRegistry` |

---

## Proto Service Definition

```protobuf
service IndexerService {
  rpc StreamEvents(EventStreamRequest)   returns (stream EventStreamResponse);
  rpc GetCompileStatus(CompileStatusRequest) returns (CompileStatusResponse);
  rpc Deploy(DeployRequest)              returns (DeployResponse);
  rpc HealthCheck(HealthRequest)         returns (HealthResponse);
}
```

---

## Integration

### Starting the gRPC server alongside Express

```js
// backend/src/server.js
import { startGrpcServer } from './grpc/index.js';

const compileJobStore = new Map();
const deployJobStore = new Map();

await startGrpcServer(compileJobStore, deployJobStore, {
  host: '0.0.0.0',
  port: Number(process.env.GRPC_PORT ?? 50051),
});
```

### Publishing a contract event to connected gRPC clients

```js
import { grpcEventBus } from './grpc/index.js';

grpcEventBus.emit('contract_event', {
  contract_id: 'C...',
  event_type: 'transfer',
  ledger_seq: '1234567',
  timestamp: Math.floor(Date.now() / 1000),
  payload_json: JSON.stringify({ from, to, amount }),
});
```

### Consuming the event stream from a client

```js
import { GrpcClient } from './grpc/index.js';

const client = new GrpcClient({ host: 'localhost', port: 50051 });
const stream = client.streamEvents({ contractId: 'C...' });

stream.on('data', ({ event }) => console.log('event:', event));
stream.on('error', (err) => console.error('stream error:', err));
```

---

## Service Discovery

```js
import { serviceRegistry } from './grpc/index.js';

// Register the indexer at startup
serviceRegistry.register('indexer', { host: 'indexer', port: 50051 });
serviceRegistry.start(); // begin health polling

// Resolve a healthy endpoint (round-robin)
const endpoint = serviceRegistry.resolve('indexer');
// → { host: 'indexer', port: 50051 }
```

---

## Environment Variables

| Variable            | Default     | Description                     |
| ------------------- | ----------- | ------------------------------- |
| `GRPC_PORT`         | `50051`     | Port the gRPC server binds to   |
| `GRPC_INDEXER_HOST` | `localhost` | Indexer host used by GrpcClient |
| `GRPC_INDEXER_PORT` | `50051`     | Indexer port used by GrpcClient |

---

## Security

- TLS is supported on both server and client via the `tls`, `certChain`, and `privateKey` options.
- Default (development) mode uses insecure credentials; set `tls: true` for production.
- The `ServiceRegistry` evicts stale entries after a configurable TTL (default 120 s).

---

## Performance Settings

| Setting                 | Value | Notes                                                     |
| ----------------------- | ----- | --------------------------------------------------------- |
| Max message size        | 64 MB | Accommodates large WASM binaries                          |
| Keepalive ping interval | 10 s  | Detects dead connections quickly                          |
| Keepalive timeout       | 5 s   |                                                           |
| Client retry limit      | 3     | Exponential backoff, UNAVAILABLE / DEADLINE_EXCEEDED only |
