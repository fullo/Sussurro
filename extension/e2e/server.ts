/* Local server for the capture harness: the two-peer call page, its
 * WebSocket signalling, and a fake Sussurro app — `GET /app/version` and
 * `WS /live` with the app's checks (extension Origin, token) — that parses
 * the audio frames and measures each channel. Loopback only. */
import http from "node:http";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { Duplex } from "node:stream";
import { WebSocketServer, type WebSocket } from "ws";
import { decodeFrame } from "../src/shared/frame.ts";

const HERE = fileURLToPath(new URL(".", import.meta.url));
const STATIC: Record<string, [string, string]> = {
  "/call.html": ["call.html", "text/html"],
  "/call.js": ["call.js", "text/javascript"],
};

export interface Channel {
  frames: number;
  samples: number;
  sumSq: number;
  firstSeq: number;
  lastSeq: number;
  seqGaps: number;
  /** The last two frames (~85 ms), for the tone check. */
  tail: Int16Array[];
}

export interface LiveSession {
  origin: string;
  start: Record<string, unknown> | null;
  controls: Record<string, unknown>[];
  channels: Map<number, Channel>;
  badFrames: number;
  stopped: boolean;
  closed: boolean;
}

const isExtensionOrigin = (o: string | undefined) => !!o && /^(chrome-extension|moz-extension):\/\/[A-Za-z0-9-]+$/.test(o);

function reject(socket: Duplex, status: number) {
  socket.write(`HTTP/1.1 ${status} ${http.STATUS_CODES[status]}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
  socket.destroy();
}

export async function startServer(token: string) {
  const sessions: LiveSession[] = [];
  const rooms = new Map<string, Set<WebSocket>>();

  const server = http.createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const origin = req.headers.origin;
    if (url.pathname === "/app/version") {
      if (origin && !isExtensionOrigin(origin)) return void res.writeHead(403).end();
      const cors = isExtensionOrigin(origin) ? { "Access-Control-Allow-Origin": origin!, Vary: "Origin" } : {};
      if (req.method === "OPTIONS") {
        return void res
          .writeHead(204, { ...cors, "Access-Control-Allow-Headers": "Authorization", "Access-Control-Allow-Methods": "GET, POST" })
          .end();
      }
      if (req.headers.authorization !== `Bearer ${token}`) return void res.writeHead(401, cors).end();
      return void res.writeHead(200, { ...cors, "Content-Type": "application/json" }).end(JSON.stringify({ app: "e2e", protocol: 2, protocol_min: 1 }));
    }
    const file = STATIC[url.pathname];
    if (!file) return void res.writeHead(404).end();
    res.writeHead(200, { "Content-Type": file[1], "Cache-Control": "no-store" }).end(readFileSync(HERE + file[0]));
  });

  const wss = new WebSocketServer({ noServer: true });
  server.on("upgrade", (req, socket, head) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    if (url.pathname === "/signal") {
      wss.handleUpgrade(req, socket, head, (ws) => {
        const room = url.searchParams.get("room") ?? "r";
        const peers = rooms.get(room) ?? new Set();
        rooms.set(room, peers);
        peers.add(ws);
        ws.on("message", (m) => {
          for (const p of peers) if (p !== ws && p.readyState === 1) p.send(m.toString());
        });
        ws.on("close", () => peers.delete(ws));
      });
      return;
    }
    if (url.pathname !== "/live") return reject(socket, 404);
    // The app's checks (sussurro/src-tauri/src/api/auth.rs): extension
    // origin first, then the token.
    if (!isExtensionOrigin(req.headers.origin)) return reject(socket, 403);
    if (url.searchParams.get("token") !== token) return reject(socket, 401);
    wss.handleUpgrade(req, socket, head, (ws) => onLive(ws, req.headers.origin!));
  });

  function onLive(ws: WebSocket, origin: string) {
    const s: LiveSession = { origin, start: null, controls: [], channels: new Map(), badFrames: 0, stopped: false, closed: false };
    sessions.push(s);
    const reply = (m: object) => ws.send(JSON.stringify({ type: "status", ...m }));
    reply({ state: "ready", protocol: 2, app: "e2e" });
    ws.on("message", (data, isBinary) => {
      if (!isBinary) {
        const m = JSON.parse(data.toString());
        s.controls.push(m);
        if (m.type === "start") {
          s.start = m;
          reply({ state: "started", item_id: `e2e-${sessions.length}` });
        } else if (m.type === "stop") {
          s.stopped = true;
          reply({ state: "done", item_id: `e2e-${sessions.length}` });
          ws.close(1000);
        } else if (m.type === "ping") {
          reply({ state: s.start ? "recording" : "ready" });
        }
        return;
      }
      if (!s.start) {
        s.badFrames++;
        return;
      }
      let f;
      try {
        f = decodeFrame(new Uint8Array(data as Buffer));
      } catch {
        s.badFrames++;
        return;
      }
      if (f.channel > 1) {
        s.badFrames++;
        return;
      }
      let c = s.channels.get(f.channel);
      if (!c) {
        c = { frames: 0, samples: 0, sumSq: 0, firstSeq: f.seq, lastSeq: f.seq - 1, seqGaps: 0, tail: [] };
        s.channels.set(f.channel, c);
      }
      if (f.seq !== c.lastSeq + 1) c.seqGaps++;
      c.lastSeq = f.seq;
      c.frames++;
      c.samples += f.pcm.length;
      for (const v of f.pcm) c.sumSq += (v / 32768) ** 2;
      c.tail.push(f.pcm);
      if (c.tail.length > 2) c.tail.shift();
    });
    ws.on("close", () => (s.closed = true));
  }

  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  const port = (server.address() as { port: number }).port;
  return {
    port,
    sessions,
    close: () =>
      new Promise<void>((r) => {
        for (const c of wss.clients) c.terminate();
        server.closeAllConnections();
        server.close(() => r());
      }),
  };
}

/** Share of a signal's energy near `freq` (Goertzel), in [0, 1]. */
export function toneShare(pcm: Int16Array[], rate: number, freq: number): number {
  const x = new Float32Array(pcm.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of pcm) for (const v of p) x[o++] = v / 32768;
  let energy = 0;
  for (const v of x) energy += v * v;
  if (!energy) return 0;
  const k = 2 * Math.cos((2 * Math.PI * freq) / rate);
  let s1 = 0;
  let s2 = 0;
  for (const v of x) {
    const s = v + k * s1 - s2;
    s2 = s1;
    s1 = s;
  }
  const power = s1 * s1 + s2 * s2 - k * s1 * s2;
  return (2 * power) / (energy * x.length);
}

export const rms = (c: Channel) => (c.samples ? Math.sqrt(c.sumSq / c.samples) : 0);
