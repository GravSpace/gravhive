const http = require('http');

// Use ephemeral port (0) so multiple standalone workers can listen concurrently without EADDRINUSE collisions
const port = process.env.PORT ? parseInt(process.env.PORT) : 0;
const server = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'text/plain' });
    res.end('Hello from benchmark worker\n');
});

server.listen(port, () => {
    // Keep alive
});
