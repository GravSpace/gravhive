const http = require('http');

const port = process.env.PORT || 3000;
const workerId = process.env.GHIVE_WORKER_ID || '0';

const server = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({
        status: 'ok',
        workerId: workerId,
        pid: process.pid,
        uptime: process.uptime()
    }));
});

server.listen(port, () => {
    console.log(`[Worker #${workerId}] HTTP server running on port ${port} (PID: ${process.pid})`);
});

