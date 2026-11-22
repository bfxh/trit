// 非 Python 客户端：Node.js 手写 pg 协议，测并发吞吐与延迟分位。
// 用法：node scripts/perf/node_client.js --bin <path> --port 15541 --clients 8 --seconds 30
const net = require('net');
const path = require('path');
const { spawn, spawnSync } = require('child_process');
const fs = require('fs');
const os = require('os');

function arg(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

const BIN = arg('bin', path.join(__dirname, '..', '..', 'target', 'release',
  process.platform === 'win32' ? 'trit-orch.exe' : 'trit-orch'));
const PORT = Number(arg('port', '15541'));
const CLIENTS = Number(arg('clients', '8'));
const SECONDS = Number(arg('seconds', '30'));
const WAL = path.join(os.tmpdir(), `perf-node-${Date.now()}.wal`);

function startupPacket(user = 'u') {
  const params = Buffer.from(`user\0${user}\0\0`, 'utf8');
  const body = Buffer.alloc(4 + params.length);
  body.writeInt32BE(196608, 0);
  params.copy(body, 4);
  const head = Buffer.alloc(4);
  head.writeInt32BE(body.length + 4, 0);
  return Buffer.concat([head, body]);
}

function queryPacket(sql) {
  const payload = Buffer.from(sql + '\0', 'utf8');
  const head = Buffer.alloc(5);
  head.write('Q', 0, 'ascii');
  head.writeInt32BE(payload.length + 4, 1);
  return Buffer.concat([head, payload]);
}

function tryParse(buf) {
  if (buf.length < 5) return null;
  const type = String.fromCharCode(buf[0]);
  const len = buf.readInt32BE(1);
  if (buf.length < 1 + len) return null;
  return { type, total: 1 + len, body: buf.subarray(5, 1 + len) };
}

function makeReader(socket) {
  let buf = Buffer.alloc(0);
  const waiters = [];
  socket.on('data', (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    while (waiters.length) {
      const msg = tryParse(buf);
      if (!msg) break;
      buf = buf.subarray(msg.total);
      waiters.shift()(msg);
    }
  });
  return () =>
    new Promise((resolve) => {
      const msg = tryParse(buf);
      if (msg) {
        buf = buf.subarray(msg.total);
        resolve(msg);
      } else {
        waiters.push(resolve);
      }
    });
}

function event(seq) {
  return {
    schema: 1, id: `t:${seq}`, trace_id: 't', span_id: `span-${seq}`, parent: null, seq,
    kind: 'task_selected', payload: null, causation_id: null, correlation_id: 'c',
    logical: { revision: 0, epoch: 0, fence: 0, round: 0, attempt: 0 },
    event_time: null, observed_at: 100 + seq, recorded_at: 100 + seq, actor: 'Host',
    payload_hash: '', prev_hash: '', chain_seq: 0,
  };
}

async function client(stats) {
  const socket = net.connect(PORT, '127.0.0.1');
  await new Promise((res, rej) => {
    socket.on('connect', res);
    socket.on('error', rej);
  });
  const read = makeReader(socket);
  socket.write(startupPacket());
  for (;;) {
    const msg = await read();
    if (msg.type === 'Z') break;
  }
  const lat = [];
  const deadline = Date.now() + SECONDS * 1000;
  while (Date.now() < deadline) {
    const t0 = process.hrtime.bigint();
    socket.write(queryPacket('SELECT count(*) AS n FROM trit_trace'));
    for (;;) {
      const msg = await read();
      if (msg.type === 'E') throw new Error('pg error');
      if (msg.type === 'Z') break;
    }
    lat.push(Number(process.hrtime.bigint() - t0) / 1e6);
  }
  socket.end();
  stats.push(lat);
}

function percentile(sorted, fraction) {
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))];
}

async function main() {
  for (const suffix of ['', '.lock', '.timeidx', '.sqlite', '.sqlite-wal', '.sqlite-shm']) {
    try {
      fs.unlinkSync(WAL + suffix);
    } catch {}
  }
  const events = [];
  for (let i = 1; i <= 10; i += 1) events.push(event(i));
  spawnSync(BIN, ['trace', '--trace-backend', 'tritdb'], {
    input: JSON.stringify({ action: 'append', path: WAL, events }),
  });
  spawnSync(BIN, ['trace', '--trace-backend', 'tritdb'], {
    input: JSON.stringify({ action: 'sync', path: WAL }),
  });

  const serve = spawn(BIN, ['trace', '--trace-backend', 'tritdb']);
  serve.stdin.write(JSON.stringify({ action: 'serve', path: WAL, listen: `127.0.0.1:${PORT}` }));
  serve.stdin.end();
  await new Promise((res) => setTimeout(res, 2000));

  const started = Date.now();
  const stats = [];
  await Promise.all(Array.from({ length: CLIENTS }, () => client(stats)));
  const elapsed = (Date.now() - started) / 1000;
  serve.kill();

  const all = stats.flat().sort((a, b) => a - b);
  const total = all.length;
  console.log(
    `node 客户端 ${CLIENTS} 连接 x ${SECONDS}s: 请求 ${total} qps=${(total / elapsed).toFixed(0)} ` +
      `p50=${percentile(all, 0.5).toFixed(3)}ms p99=${percentile(all, 0.99).toFixed(3)}ms ` +
      `p999=${percentile(all, 0.999).toFixed(3)}ms max=${all[all.length - 1].toFixed(2)}ms`,
  );
  for (const suffix of ['', '.lock', '.timeidx', '.sqlite', '.sqlite-wal', '.sqlite-shm']) {
    try {
      fs.unlinkSync(WAL + suffix);
    } catch {}
  }
  process.exit(0);
}

main().catch((error) => {
  console.error('node 客户端失败:', error.message);
  process.exit(1);
});
