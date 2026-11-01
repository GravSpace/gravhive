# Product Requirement Document (PRD)
## Gravhive (`ghive`): Next-Generation High-Performance Production Process Manager in Rust

---

### Informasi Dokumen
- **Nama Produk:** Gravhive
- **CLI Command:** `ghive`
- **Daemon Binary:** `ghived`
- **Bahasa Implementasi:** Rust (Edition 2021+)
- **Target OS:** Linux (x86_64, aarch64), macOS (Apple Silicon & Intel), Windows (x86_64)
- **Status:** Draft / Initial Architecture Specification
- **Inspirasi Utama:** PM2 (Node.js Process Manager), Supercronic, Systemd, Tini

---

## 1. Executive Summary & Visi Produk

### 1.1 Latar Belakang
Selama bertahun-tahun, **PM2** telah menjadi standar *de-facto* process manager untuk runtime JavaScript (Node.js). Namun, PM2 memiliki sejumlah kelemahan struktural di lingkungan produksi modern:
1. **Overhead Sumber Daya Tinggi:** Daemon PM2 ditulis dalam Node.js dan V8 Engine, yang mengonsumsi 50MB–150MB+ RAM hanya untuk daemon pengawas itu sendiri, dengan latensi Garbage Collection (GC).
2. **Ketergantungan Ekosistem (Dependency Bloat):** Menjalankan aplikasi native binary (Rust, Go, C++), PHP, atau Bun tetap memerlukan instalasi runtime Node.js dan ratusan dependency npm di server target.
3. **Cluster Mode Terikat Bahasa:** Mode cluster bawaan PM2 sangat bertumpu pada modul `cluster` bawaan Node.js, sehingga tidak dapat digunakan secara optimal oleh runtime lain (Bun, Go, Rust, Python, PHP).
4. **Keamanan & Kestabilan:** PM2 rentan terhadap crash runtime V8, unhandled promise rejections, serta rentang permukaan serangan (attack surface) paket npm.

### 1.2 Visi & Nilai Tambah Gravhive
**Gravhive** adalah process manager tingkat produksi (*production-grade*) yang dibangun 100% menggunakan **Rust**. Gravhive dirancang untuk memberikan keandalan absolut, performa tinggi, footprint memori minimal, dan dukungan runtime universal secara *out-of-the-box*:
- **Zero Runtime Dependencies:** Berupa single static/dynamically linked native binary. Tidak butuh Node.js, Python, atau interpreter eksternal.
- **Ultra-Low Memory Footprint:** Daemon (`ghived`) berjalan dengan konsumsi memori resident (RSS) < 10–15 MB.
- **Sub-millisecond IPC:** Komunikasi CLI-Daemon melalui Unix Domain Sockets (UDS) / Named Pipes berbasis binary serialization (Bincode/Cap'n Proto) yang aman dan instan.
- **Universal Multi-Core Cluster:** Mendukung `SO_REUSEPORT` kernel-level load balancing serta internal high-performance reverse proxy (Tokio/Hyper) yang bekerja transparan untuk semua runtime (Node.js, Bun, Go, PHP/FrankenPHP, Python, Ruby, dll).

---

## 2. Sasaran & Target Pengguna

### 2.1 Target Pengguna
1. **DevOps & Platform Engineers:** Mengelola mikro-layanan mandiri, VM/VPS (bare-metal, EC2, DigitalOcean, Hetzner), serta container minimalis.
2. **Backend Developers (Node.js, Bun, Rust, Go, PHP):** Developer yang membutuhkan workflow mudah seperti `pm2 start/reload/logs` tanpa ribet konfigurasi systemd manual.
3. **Resource-Constrained Environments:** Server spesifikasi rendah (1 vCPU, 512MB RAM), IoT/Edge computing, di mana PM2 memakan persentase RAM yang terlalu besar.

### 2.2 Metrik Keberhasilan (Success Metrics)
- **Memory Footprint Daemon:** $\le 15 \text{ MB}$ RSS saat idle, $\le 30 \text{ MB}$ saat mengawasi 50 proses.
- **Overhead CPU Daemon:** $\le 0.1\%$ CPU saat idle.
- **Zero-Downtime Reload Speed:** Pergantian proses rolling update tanpa ada paket TCP yang di-drop.
- **Cold Boot Time:** Daemon menyala dan memulihkan proses dalam waktu $< 50 \text{ ms}$.

---

## 3. Arsitektur Sistem

```
+-------------------------------------------------------------+
|                      User / CI-CD / Terminal                 |
|                               |                             |
|                        `ghive` (CLI Client)                 |
+-------------------------------+-----------------------------+
                                |  Unix Domain Socket (UDS)
                                |  (Fast Binary Protocol / IPC)
+-------------------------------v-----------------------------+
|                     `ghived` (Core Daemon)                   |
|                                                             |
|  +---------------------+   +-----------------------------+  |
|  | Process Supervisor  |   | Resource Watchdog           |  |
|  | - Spawn / Kill      |   | - sysinfo / cgroups v2      |  |
|  | - Crash Recovery    |   | - Max Memory Checker        |  |
|  | - Zombie Reaper     |   | - CPU Usage Tracker         |  |
|  +---------------------+   +-----------------------------+  |
|                                                             |
|  +---------------------+   +-----------------------------+  |
|  | Cluster Engine      |   | Log Aggregator              |  |
|  | - SO_REUSEPORT      |   | - Non-blocking ring buffer  |  |
|  | - L4/L7 Proxy Balancer  | - Auto log-rotation / gzip  |  |
|  +---------------------+   +-----------------------------+  |
|                                                             |
|  +---------------------+   +-----------------------------+  |
|  | File Watcher Engine |   | Plugin / WASM Sandbox       |  |
|  | - notify (kqueue/   |   | - Wasmtime Extensibility    |  |
|  |   inotify/events)   |   | - Webhooks & Alerting       |  |
|  +---------------------+   +-----------------------------+  |
+-------------------------------+-----------------------------+
                                |  OS Syscalls (fork/exec/kill)
     +--------------------------+--------------------------+
     |                          |                          |
+----v---------------+  +-------v------------+  +----------v---------+
| Worker 1 (Node.js) |  | Worker 2 (Bun.sh)  |  | Worker 3 (Binary)  |
+--------------------+  +--------------------+  +--------------------+
```

### 3.1 Komponen Arsitektur Utama
1. **`ghive` (CLI):** Antarmuka pengguna terminal yang ringan. Mengirim perintah terstruktur ke daemon, menerima status streaming, menampilkan tabel dan TUI (*Terminal User Interface*).
2. **`ghived` (Daemon Engine):** Proses latar belakang yang bertanggung jawab atas *lifecycle* proses anak, *health check*, pengumpulan metrik, dan *signal forwarding*.
3. **IPC Layer:** Unix Domain Socket (Linux/macOS: `/var/run/gravhive/ghive.sock` atau `~/.gravhive/ghive.sock`) dan Named Pipes (Windows). Komunikasi berbasis framing async dengan serde serialization.
4. **State Persistence Store:** SQLite lokal atau Atomic JSON snapshot (`~/.gravhive/dump.state`) untuk pemulihan cepat setelah reboot.

---

## 4. Spesifikasi Fungsional & Fitur Utama

### 4.1 Eksekusi Proses Langsung & Alur Kerja Setara PM2 (Direct CLI Workflow)
Gravhive dirancang dengan filosofi *zero-friction developer experience* yang setara atau melebihi PM2. Pengguna dapat langsung meluncurkan, memantau, dan mengelola proses langsung melalui perintah baris (CLI) tanpa harus membuat file konfigurasi terlebih dahulu.

#### 4.1.1 `ghive start`: Peluncuran Proses Langsung & Smart Runtime Detection
- **Auto-Spawn Background Daemon:** Jika daemon `ghived` belum aktif saat perintah dijalankan, CLI akan secara otomatis meluncurkan daemon di latar belakang tanpa mengharuskan pengguna menjalankan service terpisah terlebih dahulu.
- **Deteksi Runtime Otomatis (Smart Runtime Detection):**
  - `ghive start app.js` $\rightarrow$ otomatis mendeteksi runtime `node`.
  - `ghive start server.ts` $\rightarrow$ otomatis mendeteksi runtime `bun` (fallback: `deno` atau `ts-node`).
  - `ghive start ./target/release/api` $\rightarrow$ mendeteksi sebagai native binary ELF/Mach-O dan dieksekusi langsung tanpa interpreter.
  - `ghive start "php -S 0.0.0.0:8000 router.php"` $\rightarrow$ mendeteksi perintah shell/PHP.
  - `ghive start script.py` $\rightarrow$ otomatis mendeteksi interpreter `python3`.
  - `ghive start ghive.toml` atau `ghive start ecosystem.config.json` $\rightarrow$ otomatis memuat file konfigurasi multi-aplikasi.
- **Flag Eksekusi Inline (One-liner Power):**
  - `--name <name>`: Menetapkan nama unik untuk aplikasi.
  - `-i, --instances <n|max>`: Menjalankan dalam mode cluster (multi-core).
  - `--watch`: Mengaktifkan pemantauan perubahan file otomatis.
  - `--max-memory-restart <size>`: Ambang batas memori untuk auto-restart (contoh: `250MB`, `1GB`).
  - `--env <KEY=VAL>`: Injeksi variabel lingkungan secara langsung.
  - `--interpreter <bin>`: Menentukan interpreter khusus (misal: `/usr/bin/pypy3`).
  - `--cwd <path>`: Menentukan direktori kerja proses.
  - `--time`: Menambahkan prefix timestamp pada setiap baris log.
  - `--cron <expr>`: Menjalankan proses terjadwal berbasis ekspresi cron.

Contoh perintah:
```bash
# Menjalankan aplikasi Node.js sederhana
ghive start app.js --name "api-gateway"

# Menjalankan aplikasi Bun di semua core CPU (Cluster Mode) dengan limit memori
ghive start server.ts -i max --name "bun-web" --max-memory-restart 500MB

# Menjalankan binary Rust hasil kompilasi dengan watch & restart
ghive start ./target/release/order-service --name "orders" --watch

# Menjalankan script PHP bawaan
ghive start "php -S 0.0.0.0:8080 -t public" --name "php-app"
```

#### 4.1.2 `ghive stop`: Penghentian Proses Terkendali (Graceful Shutdown)
- Sintaks: `ghive stop <id | name | all>`
- Daemon mengirimkan sinyal `SIGINT` / `SIGTERM` ke proses target untuk memberikan kesempatan menyelesaikan request in-flight.
- Status proses diubah menjadi `stopped`, namun konfigurasi dan metadata tetap disimpan dalam registry agar dapat dinyalakan kembali kapan saja.

#### 4.1.3 `ghive restart` & `ghive reload`: Pemulihan & Rolling Update
- **`ghive restart <id | name | all>`:** Mematikan proses aktif lalu menyalakannya kembali secara instan.
- **`ghive reload <id | name | all>`:** Khusus aplikasi dalam mode cluster, melakukan *zero-downtime rolling reload* (pekerja baru dinyalakan dan dipastikan siap sebelum pekerja lama dimatikan).

#### 4.1.4 `ghive delete`: Penghapusan Entri Proses
- Sintaks: `ghive delete <id | name | all>`
- Menghentikan proses jika sedang berjalan dan menghapus seluruh entri serta riwayat metrik dari memori daemon.

#### 4.1.5 `ghive list` / `ghive ls`: Tabel Status Visual Real-time
- Menampilkan status seluruh proses dalam tabel CLI berformat indah, cepat, dan berwarna (ANSI colors) mirip PM2.
- Informasi kolom:
  - `ID`: ID numerik unik per proses/worker.
  - `Name`: Nama aplikasi.
  - `Namespace`: Pengelompokan aplikasi (default: `default`).
  - `Version`: Versi aplikasi (diambil otomatis dari `package.json` atau `Cargo.toml` jika ada).
  - `Mode`: `fork` atau `cluster`.
  - `PID`: Process ID OS sesungguhnya.
  - `Uptime`: Lama waktu proses menyala (contoh: `2D 4h`, `15m`).
  - `↺`: Jumlah restart / crash recovery.
  - `Status`: `online` (hijau), `stopping` (kuning), `stopped` (merah), `errored` (merah terang).
  - `CPU`: Persentase beban CPU riil.
  - `Mem`: Penggunaan RAM terkini (MB/GB).
  - `User`: User OS yang menjalankan proses.

#### 4.1.6 `ghive ecosystem` / `ghive init`: Generator Konfigurasi Otomatis
- Sintaks: `ghive ecosystem [toml | yaml | json]` atau `ghive init`
- Menghasilkan file konfigurasi standar siap pakai yang terstruktur rapi (`ghive.toml` atau `ecosystem.config.json` untuk pengguna migrasi dari PM2).
- Menghadirkan template terkonfigurasi untuk Node.js, Bun, Rust native binary, dan PHP.

#### 4.1.7 `ghive monit`: Interactive TUI Dashboard Real-Time
- Dibangun dengan library Rust `ratatui` + `crossterm`.
- Memberikan antarmuka terminal interaktif layar penuh (*terminal dashboard*) yang menampilkan:
  - **Daftar Proses & Status:** Navigasi interaktif menggunakan tombol panah keyboard `[↑]` dan `[↓]`.
  - **Gauge & Sparkline Real-time:** Grafik mini penggunaan CPU dan Memory per proses yang dipilih.
  - **Process Metadata Panel:** Informasi lengkap PID, script path, interpreter, env vars, dan statistik restart.
  - **Live Log Stream:** Jendela streaming log stdout & stderr real-time untuk proses yang sedang di-highlight.
  - **Shortcut Cepat:** `[R]` untuk restart proses yang dipilih, `[S]` untuk stop, `[D]` untuk delete, `[Q]` untuk keluar dari TUI.

---

### 4.2 Auto-Restart & Startup Scripts
- **Crash Detection & Auto-Restart:**
  - Menangkap sinyal terminasi (`SIGCHLD`, exit code $\ne 0$).
  - Fitur **Exponential Backoff with Jitter**: Jika aplikasi crash berulang kali dalam interval singkat (< 1 detik), jeda restart akan ditingkatkan secara otomatis (misal: 100ms -> 500ms -> 2s -> 10s) untuk menghindari CPU thrashing / spin loop.
  - Opsi `max_restarts` dan `min_uptime` untuk menandai proses yang *broken* (*Errored* state).
- **System Startup Generator:**
  - `ghive startup`: Mendeteksi sistem init host secara otomatis (Systemd, OpenRC, macOS Launchd, Windows Service).
  - Menghasilkan file konfigurasi unit (misal: `/etc/systemd/system/gravhive.service`) yang menyalakan daemon `ghived` saat OS booting.
  - `ghive save`: Menyimpan seluruh daftar aplikasi yang sedang aktif beserta konfigurasinya ke file dump lokal.
  - `ghive resurrect`: Menghidupkan kembali seluruh konfigurasi proses yang disimpan sebelumnya.

### 4.3 Cluster Mode (Multi-Core CPU Scaling)
- **Arsitektur Dual Cluster:**
  1. **Mode A: Native Kernel `SO_REUSEPORT`:**
     - Untuk runtime modern (Node.js v18+, Bun, Go, Rust, FrankenPHP), Gravhive menginjeksi environment variable dan mengizinkan setiap worker process mengikat (*bind*) ke port dan IP yang sama menggunakan flag `SO_REUSEPORT`. Kernel Linux/BSD akan mendistribusikan koneksi TCP secara merata tanpa overhead user-space proxy.
  2. **Mode B: Built-in High-Performance Reverse Proxy (Tokio L4/L7):**
     - Untuk aplikasi konvensional atau yang tidak mendukung port sharing, `ghived` membuka port publik dan bertindak sebagai load balancer ultra-cepat (round-robin / least connections), mendistribusikan lalu lintas ke internal port/UDS masing-masing worker.
- **Worker Management:**
  - Opsi instans: `instances: "max"` (sesuai jumlah core logika CPU) atau angka spesifik (misal: `instances: 4`).
  - Environment variable unik per worker: `GHIVE_WORKER_ID`, `GHIVE_INSTANCE_INDEX`, `PORT`.

### 4.4 Zero-Downtime Hot Reload
- **Rolling Restart Flow:**
  1. Daemon meluncurkan worker versi baru ($W_{new}$) berdampingan dengan worker lama ($W_{old}$).
  2. Daemon menjalankan **Readiness Probe**:
     - Mode HTTP: Polling endpoint (misal: `GET /health` sampai status 200).
     - Mode TCP: Memastikan port worker telah listening.
     - Mode IPC: Worker mengirim sinyal *ready* ke daemon (`ghive.ready()`).
  3. Setelah $W_{new}$ dinyatakan *Healthy*, daemon mengirimkan sinyal `SIGTERM` / `SIGINT` ke $W_{old}$ dengan toleransi grace period (default 5000ms).
  4. Selama grace period, koneksi in-flight tetap diselesaikan.
  5. Jika $W_{old}$ belum berhenti setelah timeout, paksa dengan `SIGKILL`.
  6. Proses diulang secara bergantian untuk worker berikutnya hingga seluruh cluster terbarukan tanpa *dropped connection*.

### 4.5 Max Memory Reload & Resource Watchdog
- **Threshold Watchdog:**
  - Daemon mengamati metrik memori secara periodik (polling interval dapat diatur, default tiap 3 detik).
  - Mengambil data melalui Linux `cgroups v2` (`memory.current`) atau Rust crate `sysinfo` untuk performa tinggi tanpa memicu pemanggilan eksternal seperti `ps` atau `top`.
- **Soft vs Hard Restart:**
  - Jika `memory > max_memory_restart` (misal: `300MB`):
    - Memberikan log peringatan: `[WATCHDOG] Memory limit exceeded (312MB > 300MB). Initiating graceful reload...`
    - Memicu mekanisme graceful reload yang sama seperti *hot reload* sehingga tidak mengganggu user yang sedang aktif.

### 4.6 Watch & Reload (Development Mode)
- **In-Memory File System Watcher:**
  - Menggunakan library Rust `notify` yang mengabstraksi mekanisme kernel real-time (`inotify` di Linux, `kqueue`/`FSEvents` di macOS, `ReadDirectoryChangesW` di Windows).
- **Debounce Timer & Smart Filters:**
  - Menghindari reload bertubi-tubi saat proses build menulis banyak file secara bersamaan melalui debounce buffer (default: 300ms).
  - Dukungan ignore-pattern berbasis glob (`.gitignore` aware, mengabaikan `.git/**`, `node_modules/**`, `vendor/**`, `*.log`, file output build).

### 4.7 Log Management & Rotation
- **High-Throughput Async Pipe:**
  - Stdout dan Stderr setiap proses ditangkap melalui asynchronous pipe tanpa memblokir I/O aplikasi.
  - Setiap baris log diberi metadata: Timestamp (ISO 8601/RFC 3339), App Name, Worker ID, dan Log Stream (`STDOUT` / `STDERR`).
- **Auto-Rotation Bawaan:**
  - Tanpa butuh utilitas eksternal seperti `logrotate`.
  - Konfigurasi batas ukuran (misal: `max_size: 10M`) dan retensi file (misal: `retain: 5`).
  - Kompresi file log lama menggunakan algoritma Zstandard / Gzip secara asinkron di thread terpisah.
- **Log Streaming CLI:**
  - `ghive logs <app>`: Membaca log langsung dari ring buffer daemon dengan opsi filtering per worker ID atau tipe stream (`--err` / `--out`).

### 4.8 Observability & Eksternal Metrics (Prometheus)
- **Interactive TUI Dashboard (`ghive monit`):**
  - Dibangun menggunakan library TUI modern Rust (`ratatui` + `crossterm`).
  - Menampilkan panel realtime: daftar proses, status (*online*, *restarting*, *errored*), grafik visual mini (sparkline) pemakaian CPU/RAM, uptime, log console live, dan event stream.
- **Observability Eksternal:**
  - Endpoint HTTP bawaan (opsional): `/metrics` berformat OpenMetrics / Prometheus untuk integrasi dengan Grafana dan Datadog.

### 4.9 Deployment Workflow & Konfigurasi Ekosistem
- **Format Konfigurasi:**
  - Mendukung `ghive.toml` (rekomendasi standar Rust/modern), `ghive.yaml`, dan `ghive.json`.
- **Deployment Scripting:**
  - Skrip deployment multi-tahap (`pre-deploy`, `post-deploy`, `health-check`).
  - Kemampuan eksekusi remote melalui SSH yang terintegrasi (mirip `pm2 deploy`).

Contoh `ghive.toml`:
```toml
[app.api-service]
script = "bun"
args = ["run", "src/index.ts"]
cwd = "/var/www/api"
instances = "max"
exec_mode = "cluster"
autorestart = true
max_memory_restart = "256MB"
watch = false
env = { NODE_ENV = "production", PORT = "3000" }

[app.api-service.log]
out = "/var/log/gravhive/api-out.log"
err = "/var/log/gravhive/api-err.log"
rotate_max_size = "20MB"
rotate_keep = 7

[app.worker-job]
script = "./target/release/payment-worker"
exec_mode = "fork"
instances = 2
autorestart = true
restart_delay = "500ms"
max_restarts = 10
```

### 4.10 Integrasi Container & Docker (PID 1 Init Mode)
- **`ghive-runtime` / `--no-daemon` Mode:**
  - Berfungsi sebagai **PID 1 (Init System)** di dalam Docker Container.
  - Mengatasi masalah umum Docker: penumpukan *zombie processes* (melakukan `waitpid` otomatis) dan meneruskan sinyal OS (`SIGINT`, `SIGTERM`) dengan benar ke semua subproses.
  - Output log langsung diarahkan ke standard container `stdout/stderr` untuk dikonsumsi oleh Docker logging driver.

### 4.11 Module & Plugin System (WASM Sandboxing)
- **Plugin Hooks:**
  - Mengizinkan ekstensi untuk mendengarkan lifecycle event: `on_process_crash`, `on_max_memory_reload`, `on_deployment_success`.
- **WASM-Based Sandboxing (Wasmtime):**
  - Plugin dapat ditulis dalam bahasa apa pun yang meng-compile ke WebAssembly (Rust, Go, C, AssemblyScript).
  - Menjalankan plugin dengan isolasi memori total sehingga modul pihak ketiga tidak dapat merusak daemon Gravhive.
  - Contoh modul bawaan: Notifikasi Webhook (Slack, Discord, Telegram), Alert PagerDuty.

---

## 5. Referensi Command Line Interface (CLI) & User Experience

### 5.1 Tabel Perintah Lengkap (PM2-Parity Reference)

| Perintah | Argumen / Opsi | Deskripsi |
| :--- | :--- | :--- |
| **`ghive start <target>`** | `-i, --instances <n\|max>`, `--name <name>`, `--watch`, `--max-memory-restart <size>`, `--env <K=V>`, `--cwd <path>` | Menjalankan aplikasi langsung dari file script (`.js`, `.ts`, `.py`, `.php`), compiled binary, atau file konfigurasi (`ghive.toml` / `ecosystem.config.json`). Otomatis menyalakan daemon jika belum aktif. |
| **`ghive stop <id\|name\|all>`** | `--force`, `--timeout <ms>` | Menghentikan proses secara graceful via `SIGTERM` (atau paksa dengan `SIGKILL` bila `--force`). |
| **`ghive restart <id\|name\|all>`** | `--watch` | Mematikan lalu menyalakan kembali proses/cluster secara cepat. |
| **`ghive reload <id\|name\|all>`** | `--graceful-timeout <ms>` | Melakukan zero-downtime rolling reload (pekerja baru siap sebelum pekerja lama dimatikan). |
| **`ghive delete <id\|name\|all>`** | - | Menghentikan proses dan menghapus seluruh entri serta riwayat dari daftar pengawasan. |
| **`ghive list` / `ghive ls`** | `--json`, `--sort <col>`, `--mini` | Menampilkan tabel visual real-time status semua aplikasi aktif & non-aktif mirip PM2. |
| **`ghive ecosystem` / `ghive init`** | `[toml\|yaml\|json]`, `--full` | Membuat file template konfigurasi ekosistem siap pakai (`ghive.toml`, `ghive.yaml`, atau `ecosystem.config.json`). |
| **`ghive monit`** | - | Membuka TUI interactive terminal dashboard layar penuh untuk monitoring CPU, RAM, proses, dan logs. |
| **`ghive logs [id\|name]`** | `-n, --lines <n>`, `-f, --follow`, `--err`, `--out`, `--timestamps` | Melakukan streaming log dari daemon secara real-time dari non-blocking memory buffer. |
| **`ghive flush [id\|name]`** | - | Mengosongkan file log aplikasi. |
| **`ghive describe <id\|name>`** | - | Menampilkan detail teknis mendalam: PID, path, working directory, interpreter, variabel environment, batas resource, uptime, dan statistik crash. |
| **`ghive save`** | `--force` | Menyimpan daftar proses aktif saat ini ke snapshot file (`dump.state`). |
| **`ghive resurrect`** | - | Memulihkan kembali semua proses dari snapshot file yang disimpan sebelumnya. |
| **`ghive startup`** | `[systemd\|launchd\|openrc]` | Membuat dan mengonfigurasi startup script otomatis di OS agar menyala saat reboot server. |
| **`ghive unstartup`** | - | Menghapus integrasi startup service dari OS. |
| **`ghive prune`** | - | Membersihkan entri proses yang berstatus `stopped` atau `errored` serta log file usang. |

---

### 5.2 Desain Visual Terminal Output

#### 5.2.1 Mockup Tampilan `ghive list` / `ghive ls`
```
┌────┬────────────────┬───────────┬─────────┬─────────┬────────┬──────────┬────────┬──────────┬──────────┬──────────┐
│ id │ name           │ namespace │ version │ mode    │ pid    │ uptime   │ ↺      │ status   │ cpu      │ mem      │
├────┼────────────────┼───────────┼─────────┼─────────┼────────┼──────────┼────────┼──────────┼──────────┼──────────┤
│ 0  │ api-gateway    │ default   │ 1.4.2   │ cluster │ 142801 │ 4D 12h   │ 0      │ online   │ 0.4%     │ 42.1 MB  │
│ 1  │ api-gateway    │ default   │ 1.4.2   │ cluster │ 142802 │ 4D 12h   │ 0      │ online   │ 0.6%     │ 43.8 MB  │
│ 2  │ bun-service    │ backend   │ 0.9.0   │ fork    │ 142915 │ 18h 04m  │ 1      │ online   │ 0.1%     │ 31.2 MB  │
│ 3  │ rust-engine    │ core      │ 2.0.1   │ fork    │ 143004 │ 12D 01h  │ 0      │ online   │ 1.2%     │ 12.4 MB  │
│ 4  │ worker-queue   │ default   │ 1.0.0   │ fork    │ 0      │ 0        │ 4      │ stopped  │ 0.0%     │ 0 B      │
│ 5  │ php-dashboard  │ web       │ 8.2.1   │ fork    │ 143110 │ 2h 30m   │ 0      │ online   │ 0.0%     │ 24.5 MB  │
└────┴────────────────┴───────────┴─────────┴─────────┴────────┴──────────┴────────┴──────────┴──────────┴──────────┘
 [Daemon: ghived v0.1.0] [Total Memory: 154.0 MB] [Active Processes: 5/6]
```

#### 5.2.2 Mockup Tampilan TUI `ghive monit`
```
┌─ [Gravhive Monitor v0.1.0] ────────────────────────────────────────────────────────┐
│ Processes (6)                        │ Selected Process: api-gateway (ID: 0)       │
│ > [0] api-gateway      [online] 0.4% │ PID: 142801        Uptime: 4D 12h           │
│   [1] api-gateway      [online] 0.6% │ Mode: Cluster      Restarts: 0              │
│   [2] bun-service      [online] 0.1% │ Interpreter: Node  Port: 3000               │
│   [3] rust-engine      [online] 1.2% │ Script: /var/www/api/index.js               │
│   [4] worker-queue     [stopped] 0%  ├─────────────────────────────────────────────┤
│   [5] php-dashboard    [online] 0.0% │ CPU Usage: [|||||                      ] 4% │
│                                      │ Memory:    [|||||||||||||              ] 42M│
├──────────────────────────────────────┴─────────────────────────────────────────────┤
│ Live Logs (STDOUT / STDERR) - Press [L] to toggle filter                           │
│ 2026-11-01T10:25:01Z [api-gateway:0] [INFO] HTTP GET /api/v1/health 200 (1.2ms)    │
│ 2026-11-01T10:25:03Z [api-gateway:0] [INFO] HTTP POST /api/v1/auth 200 (14.5ms)   │
│ 2026-11-01T10:25:05Z [api-gateway:0] [WARN] Cache miss for key: user:session:8812  │
│ 2026-11-01T10:25:08Z [api-gateway:0] [INFO] Process memory stabilized at 42.1 MB   │
├────────────────────────────────────────────────────────────────────────────────────┤
│ [↑/↓] Select Process  | [R] Restart  | [S] Stop  | [D] Delete  | [Q] Quit          │
└────────────────────────────────────────────────────────────────────────────────────┘
```

#### 5.2.3 Output Generator `ghive ecosystem toml`
Perintah `ghive ecosystem` akan menghasilkan file `ghive.toml` dengan template siap pakai:
```toml
# Gravhive Ecosystem Configuration File (ghive.toml)
# Jalankan dengan: ghive start ghive.toml

[app.web-service]
script = "app.js"                   # Script entry point atau compiled binary
name = "web-service"                # Nama aplikasi
instances = "max"                   # "max" atau jumlah core tertentu
exec_mode = "cluster"               # "cluster" atau "fork"
autorestart = true                  # Auto-restart bila terjadi crash
max_memory_restart = "300MB"        # Restart otomatis bila melebihi batas memori
watch = false                       # Ubah ke true untuk auto-reload saat file berubah
env = { NODE_ENV = "production", PORT = "8080" }

[app.background-worker]
script = "./worker-bin"
exec_mode = "fork"
instances = 1
autorestart = true
restart_delay = "1s"
max_restarts = 10
```

---

## 6. Arsitektur Teknis Rust & Strategi Optimasi

### 6.1 Rust Crates Ecosystem
- **Async Runtime & Networking:**
  - `tokio`: Async engine multi-threaded untuk I/O socket, timer, dan process spawning non-blocking.
  - `tokio-util`: Codec framing untuk protokol IPC.
  - `hyper` / `socket2`: Konfigurasi socket tingkat rendah (`SO_REUSEPORT`, `TCP_NODELAY`, `SO_KEEPALIVE`) dan reverse-proxy.
- **Process & System Interop:**
  - `nix` / `libc`: POSIX system call bindings (`fork`, `execve`, `setuid`, `setgid`, `kill`, `waitpid`, `cgroups`).
  - `sysinfo`: Pengambilan metrik CPU, RAM, Disk, dan Process Tree lintas OS secara efisien.
- **CLI & TUI Presentation:**
  - `clap` (v4 with derive): Parsing argumen CLI yang sangat cepat dan terdokumentasi rapi.
  - `ratatui` + `crossterm`: Framework render TUI grafis terminal dengan refresh rate tinggi dan konsumsi CPU rendah.
  - `comfy-table`: Format visualisasi tabel di terminal.
- **File Watching & Serialization:**
  - `notify`: Cross-platform event-driven file system monitoring.
  - `serde`, `serde_json`, `toml`: Serialisasi dan deserialisasi data konfigurasi serta state dump.
  - `bincode`: Protokol serialisasi biner ultra-ringan untuk komunikasi internal IPC.
- **Extensibility:**
  - `wasmtime`: Engine WebAssembly sandboxing untuk sistem plugin aman.

### 6.2 Optimasi Bawaan Rust (Rust-Native Optimizations)
1. **Zero-Cost Abstractions & No Garbage Collection:**
   - Tidak ada jeda *stop-the-world* (GC pause) saat membaca log berukuran gigabyte atau mengawasi ratusan proses.
2. **Compile-Time LTO & Binary Size Stripping:**
   - Menggunakan konfigurasi `Cargo.toml`:
     ```toml
     [profile.release]
     opt-level = 3
     lto = "fat"
     codegen-units = 1
     panic = "abort"
     strip = true
     ```
   - Menghasilkan binary mandiri berukuran $\le 8 - 15 \text{ MB}$.
3. **Lock-Free Concurrency & Actor/Channel Architecture:**
   - Komunikasi internal antar task di daemon menggunakan unbounded/bounded atomic channels (`tokio::sync::mpsc`, `crossbeam-channel`).
   - Minim penggunaan `Mutex`/`RwLock` global untuk mencegah thread contention di multi-core CPU.
4. **Zero-Copy Log Buffering:**
   - Menggunakan circular memory-mapped ring buffer untuk log pipeline agar penulisan log berkecepatan tinggi tidak membebani alokator heap (`jemalloc` atau `mimalloc` sebagai global allocator).
5. **Direct Linux cgroups v2 Integration:**
   - Pengawasan pemakaian memori langsung membaca virtual filesystem `/sys/fs/cgroup/...` tanpa membuat *child process* baru untuk mengecek penggunaan RAM.

---

## 7. Persyaratan Non-Fungsional (NFR)

### 7.1 Keamanan (Security)
- **Privilege Dropping:** Daemon dapat dijalankan sebagai root (untuk `SO_REUSEPORT` atau binding port `< 1024`), namun worker anak dapat secara otomatis diturunkan ke pengguna non-root (`user: "www-data"` / `setuid`).
- **IPC Access Control:** Socket file UNIX (`ghive.sock`) diberi permission ketat (`0600` atau `0660` dengan grup terverifikasi) untuk mencegah pengguna lokal yang tidak memiliki hak akses mengontrol proses server.
- **Environment Sanitization:** Proteksi terhadap *leakage* variabel sensitif antar worker.

### 7.2 Keandalan & Robustness (Reliability)
- **Immortal Daemon:** Daemon harus menangani semua error internal secara terstruktur (`Result<T, E>`) tanpa pernah memicu *uncontrolled panic*.
- **Zombie Process Reaping:** Dalam mode container, daemon secara aktif melakukan loop `waitpid(-1, WNOHANG)` untuk mencegah resource leak akibat zombie process dari proses turunan (misal `child_process.fork()` di Node.js yang orphaned).

### 7.3 Kompatibilitas Lintas Platform
- **Linux:** Full support (Systemd, cgroups v2, epoll, `SO_REUSEPORT`).
- **macOS:** Full support untuk developer lokal (launchd, kqueue).
- **Windows:** Dukungan Windows Service dan Job Objects untuk pengelompokan proses dan pemantauan memori.

---

## 8. Roadmap & Rencana Implementasi

```
+--------------------------------------------------------------------------+
| MILESTONE 1: Core Engine & CLI MVP                                       |
| - CLI & Daemon IPC (Tokio UDS + Bincode)                                 |
| - Process Spawning, Auto-restart, Exponential Backoff                    |
| - `ghive start/stop/restart/list/delete`                                  |
| - Konfigurasi dasar `ghive.toml`                                         |
+--------------------------------------------------------------------------+
                                     |
                                     v
+--------------------------------------------------------------------------+
| MILESTONE 2: Cluster, Watchdog & Hot Reload                              |
| - Cluster Mode (SO_REUSEPORT & L4 Internal Reverse Proxy)                |
| - Zero-downtime rolling reload & HTTP Readiness Probe                    |
| - Max Memory Reload Watchdog (sysinfo + cgroups v2)                      |
| - Watch & Reload menggunakan `notify` crate                              |
+--------------------------------------------------------------------------+
                                     |
                                     v
+--------------------------------------------------------------------------+
| MILESTONE 3: Observability, Logs & TUI                                   |
| - Async Log Aggregator, File Rotation & Compression                      |
| - `ghive logs -f` real-time streaming                                    |
| - TUI Dashboard interaktif menggunakan `ratatui` (`ghive monit`)         |
| - Startup script generator (`systemd`, `launchd`)                        |
+--------------------------------------------------------------------------+
                                     |
                                     v
+--------------------------------------------------------------------------+
| MILESTONE 4: Production Polish & Extensibility                           |
| - Docker PID 1 Init runtime (`--no-daemon`)                              |
| - Remote Deployment workflow via SSH (`ghive deploy`)                    |
| - Plugin system berbasis WebAssembly (Wasmtime)                          |
| - Benchmark publik versus PM2 (CPU, RSS, Latency, RPS)                   |
+--------------------------------------------------------------------------+
```

---

## 9. Kesimpulan
**Gravhive** (`ghive`) menutup celah besar dalam ekosistem DevOps modern dengan menghadirkan kemudahan *developer experience* layaknya PM2, namun dipadukan dengan kecepatan, keandalan, efisiensi memori, dan keamanan tingkat tinggi dari bahasa pemrograman **Rust**. Gravhive menjadi solusi universal untuk mengelola proses aplikasi backend masa kini—baik Node.js, Bun, PHP, Rust, Go, maupun aplikasi native lainnya.

