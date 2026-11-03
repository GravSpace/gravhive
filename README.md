# Gravhive (`ghive`) 🚀

[![CI](https://github.com/GravSpace/gravhive/actions/workflows/ci.yml/badge.svg)](https://github.com/GravSpace/gravhive/actions/workflows/ci.yml)
[![Release](https://github.com/GravSpace/gravhive/actions/workflows/release.yml/badge.svg)](https://github.com/GravSpace/gravhive/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> **Next-Generation High-Performance Production Process Manager in Rust**  
> Terinspirasi dari PM2, dibangun dari awal dengan 100% Rust untuk keandalan produksi, zero GC overhead, footprint memori minimal, dan dukungan multi-runtime universal.

---

## ⚡ Fitur Utama

- **Zero-Dependency Native Binaries:** CLI tunggal (`ghive`) dan daemon pengawas (`ghived`). Tanpa dependensi Node.js, Python, atau V8 runtime eksternal.
- **Ultra-Low Memory Footprint:** Konsumsi memori daemon $\le 10\text{–}15\text{ MB}$ RSS (jauh lebih hemat dibanding PM2 yang memakan 50–150MB+).
- **Universal Multi-Runtime Support:** Menjalankan langsung skrip atau biner:
  - Node.js (`.js`, `.mjs`, `.cjs`)
  - Bun / Deno (`.ts`, `.mts`, `.cts`)
  - Native Compiled Binaries (Rust, Go, C++, Zig)
  - PHP (CLI, Built-in Server, PHP-FPM / FrankenPHP)
  - Python (`.py`)
- **Auto-Spawn Background Daemon:** Menjalankan perintah apa pun (seperti `ghive start`) otomatis menyalakan daemon `ghived` jika belum aktif.
- **Auto-Restart & Crash Loop Protection:** Menangkap exit code $\ne 0$ dan menerapkan _Exponential Backoff with Jitter_ untuk mencegah CPU thrashing.
- **Cluster Mode & Zero-Downtime Hot Reload:** Menjalankan worker di beberapa core CPU sekaligus (`-i max`) dan melakukan rolling update tanpa koneksi terputus (`ghive reload`).
- **Max Memory Watchdog:** Memantau konsumsi RAM setiap worker secara real-time via `sysinfo`. Merestart otomatis saat ambang batas memori terlampaui (`--max-memory-restart`).
- **Watch & Reload:** Memantau perubahan file secara event-driven via kernel inotify/kqueue (`--watch`).
- **Log Management & Streaming:** Non-blocking async ring buffer di memori serta file log terisolasi (`ghive logs <app> -f`).
- **Full-Screen TUI Dashboard (`ghive monit`):** Antarmuka terminal interaktif berbasis `ratatui` untuk memantau metrik CPU, RAM, status, dan logs secara langsung.
- **State Persistence & Startup Integration:** Snapshot konfigurasi (`ghive save` & `ghive resurrect`) dan generator service boot OS (`ghive startup` untuk Systemd & macOS Launchd).
- **Smart Alerting & Webhooks:** Notifikasi instan ke **Discord**, **Telegram**, **Slack**, dan Generic Webhook saat proses crash, memory exceeded, atau deployment result.
- **Linux Kernel Isolation (cgroups v2 & Sandbox Security):** Pembatasan memori keras (`memory.max`), kuota CPU (`cpu.max`), proteksi fork-bomb (`pids.max`), dan privilege dropping (`setuid`/`setgid`).
- **Built-in Cron & Job Scheduler:** Menjalankan script periodik berbasis ekspresi cron standar tanpa membutuhkan crontab OS (`ghive cron`).
- **Zero-Downtime Deployment Workflow:** Pipeline deployment multi-tahap (`ghive deploy <env>`) dengan pre-deploy hook, git pull, post-deploy hook, dan rollback alert.
- **Socket Activation & Scale-to-Zero:** Menghemat konsumsi RAM hingga 100% pada aplikasi idle dengan menahan port TCP dan menyalakan worker secara instan hanya saat ada koneksi masuk.
- **Extensible WASM Plugin System:** Menjalankan ekstensi pihak ketiga secara aman dalam WebAssembly sandbox (`wasmi`) dari file `.wasm` / `.wat` (`ghive plugins`).
- **Multi-Server Mesh & Centralized Management (`ghive mesh`):** Hubungkan banyak server Gravhive ke dalam jaringan mesh terpadu. Pantau agregat proses dan kesehatan CPU/RAM seluruh node dari satu terminal pusat.
- **In-Memory Secret Vault & AES-256-GCM Encryption (`ghive secrets`):** Enkripsi kuat untuk file konfigurasi `.env` (`.env.enc`) dan vault terenkripsi dengan penyamaran otomatis (_credential redaction_) di log dan `describe`.
- **On-Demand CPU & Memory Profiling (`ghive profile`):** Generator _interactive SVG Flamegraph_ bawaan tanpa dependensi luar untuk mendiagnosis bottleneck CPU dan memory leak pada aplikasi produksi secara real-time.
- **Built-in Self-Upgrade (`ghive upgrade`):** Memperbarui biner `ghive` dan `ghived` secara atomik langsung dari rilis resmi GitHub hanya dengan satu perintah tanpa perlu kompilasi ulang manual.
- **Pre-Flight Ecosystem Validator (`ghive validate` / `ghive check`):** Linter konfigurasi cerdas untuk memverifikasi sintaks, keberadaan file skrip dan biner, executable interpreter di `$PATH`, interval cron, batas memori, konflik port TCP, dan format readiness probe sebelum proses dijalankan.
- **System & Environment Diagnostics (`ghive doctor`):** Diagnostik kesehatan server komprehensif untuk memeriksa IPC daemon, socket, vault secret key, webhook alert, CPU/RAM, ulimit file descriptor, cgroups v2, runtime eksternal (Node, Bun, Deno, Python, Rust, Docker, PHP), disk log, dan port Prometheus.
- **Lifecycle & Storage Garbage Collection (`ghive prune`):** Membersihkan worker yang berstatus `stopped` atau `errored` dari memori serta menghapus berkas log usang atau orphan dengan simulasi aman (`--dry-run`).
- **Audit Journal & Event Timeline (`ghive events` / `ghive history`):** Linimasa peristiwa siklus hidup worker (spawn, exit, crash, restart, reload, scale, dan rotasi log) dengan timestamp presisi dan dukungan format JSON untuk pipeline audit CI/CD.

---

## ⚡ Performance Benchmark (Gravhive vs PM2)

Gravhive dirancang dari awal menggunakan 100% Rust murni tanpa dependensi runtime JavaScript/V8. Berikut adalah hasil pengujian komparatif otomatis pada sistem nyata:

| Metrik Evaluasi | Gravhive (100% Rust) | PM2 (Node.js / V8) | Keunggulan Gravhive |
| :--- | :---: | :---: | :---: |
| **Daemon Idle Memory (RSS)** | **12.25 MB** | 63.77 MB | **5.2x lebih hemat** |
| **Daemon Memory (4 Workers)** | **8.47 MB** | 61.77 MB | **7.3x lebih hemat** |
| **Daemon Cold Boot Time** | **131 ms** | 403 ms | **3.1x lebih cepat** |
| **Spawn 4 Workers** | **25 ms** | 484 ms | **19.4x lebih cepat** |
| **Zero-Downtime Reload (4 Workers)** | **348 ms** | 668 ms | **1.9x lebih cepat** |
| **CLI Query Round-trip Latency** | **8 ms** | 155 ms | **19.4x lebih responsif** |

> Laporan analisis teknis lengkap dapat dilihat di [BENCHMARK.md](BENCHMARK.md). Anda dapat mereproduksi pengujian performa ini secara lokal kapan saja dengan menjalankan `./benchmarks/run.sh`.

---

## 🛠️ Instalasi & Kompilasi

### 1. Instalasi Otomatis (Linux Installer) 🐧

Gunakan skrip installer otomatis untuk Linux (Ubuntu, Debian, CentOS, RHEL, Fedora, Arch, Alpine, dll.):

```bash
# Jalankan installer otomatis (mendukung system-wide sudo atau user-mode ~/.local/bin)
bash install.sh
```

**Fitur installer Linux (`install.sh`):**

- Mendeteksi arsitektur CPU (`x86_64`, `aarch64` / `arm64`).
- Memeriksa ketersediaan Linux **cgroups v2** untuk fitur isolasi memori & CPU.
- Menginstal biner `ghive` dan `ghived` ke `/usr/local/bin` (atau `~/.local/bin`).
- Mengonfigurasi, mengaktifkan, dan menyalakan systemd service otomatis (`/etc/systemd/system/gravhive.service`).
- Inisialisasi direktori runtime `~/.gravhive/` (logs & plugins).

Untuk menghapus instalasi (_uninstall_):

```bash
bash install.sh --uninstall
# atau
bash scripts/uninstall.sh
```

---

### 2. Download Pre-Built Binary dari GitHub Releases 📦

Setiap rilis tag (misal `v0.1.0`) otomatis dikompilasi oleh GitHub Actions CI/CD menjadi arsip biner native:

```bash
# Unduh arsip sesuai target OS/arsitektur CPU Anda dari GitHub Releases:
# - Linux x86_64: gravhive-x86_64-unknown-linux-gnu.tar.gz
# - Linux ARM64:  gravhive-aarch64-unknown-linux-gnu.tar.gz
# - macOS Apple Silicon: gravhive-aarch64-apple-darwin.tar.gz
# - macOS Intel: gravhive-x86_64-apple-darwin.tar.gz

tar -xzvf gravhive-x86_64-unknown-linux-gnu.tar.gz
cd gravhive-x86_64-unknown-linux-gnu
sudo ./install.sh
```

---

### 3. Kompilasi Manual dari Source

### Persyaratan

- Rust toolchain (v1.75+)

```bash
# Clone repository
git clone https://github.com/your-org/GravHive.git
cd GravHive

# Build optimized release binary
cargo build --release

# Binari siap digunakan di target/release/ghive dan target/release/ghived
sudo cp target/release/ghive /usr/local/bin/
sudo cp target/release/ghived /usr/local/bin/
```

---

## 📖 Panduan Penggunaan CLI

### 1. Menjalankan Aplikasi (`ghive start`)

```bash
# Menjalankan aplikasi Node.js sederhana
ghive start app.js --name "api-gateway"

# Menjalankan aplikasi di semua core CPU (Cluster Mode) dengan limit memori
ghive start server.ts -i max --name "bun-web" --max-memory-restart 500MB

# Menjalankan binary Rust hasil kompilasi
ghive start ./target/release/order-service --name "orders"

# Menjalankan server PHP built-in
ghive start "php -S 0.0.0.0:8080 -t public" --name "php-app"

# Menjalankan dari file konfigurasi ekosistem
ghive start ghive.toml
```

### 2. Melihat Status & Metrik Real-Time (`ghive list` / `ghive ls`)

```bash
ghive list
```

Output:

```
┌────┬────────────────┬─────────┬────────┬──────────┬────────┬──────────┬──────────┬──────────┐
│ id │ name           │ mode    │ pid    │ uptime   │ ↺      │ status   │ cpu      │ mem      │
├────┼────────────────┼─────────┼────────┼──────────┼────────┼──────────┼──────────┼──────────┤
│ 0  │ api-gateway    │ fork    │ 10502  │ 12s      │ 0      │ online   │ 0.0%     │ 28.1 MB  │
│ 1  │ task-worker    │ fork    │ 10520  │ 12s      │ 0      │ online   │ 0.0%     │ 9.9 MB   │
└────┴────────────────┴─────────┴────────┴──────────┴────────┴──────────┴──────────┴──────────┘
 ● [Daemon: ghived v0.1.0] [Active: 2/2] [Total Memory: 38.0 MB]
```

### 3. Mengontrol Proses (`stop`, `restart`, `reload`, `delete`)

```bash
# Menghentikan proses
ghive stop api-gateway
ghive stop 0
ghive stop all

# Merestart proses
ghive restart api-gateway
ghive restart all

# Zero-downtime rolling reload (khusus cluster mode)
ghive reload bun-web

# Kustomisasi reload: jeda stabilisasi (ms) dan batas graceful shutdown (ms)
ghive reload bun-web --delay 50 --timeout 3000

# Dynamic Process Scaling (tambah/kurang worker runtime tanpa stop aplikasi)
ghive scale api-gateway 4    # Set tepat 4 worker
ghive scale api-gateway +2   # Tambah 2 worker secara instan
ghive scale api-gateway -1   # Kurangi 1 worker secara graceful

# Menghapus proses dari pengawasan
ghive delete api-gateway
ghive delete all
```

### 4. Interactive TUI Monitor (`ghive monit`)

```bash
ghive monit
```

_Gunakan tombol `[↑/↓]` untuk navigasi proses, `[R]` restart, `[S]` stop, `[D]` delete, dan `[Q]` untuk keluar._

### 5. Manajemen Log & Rotasi Otomatis (`ghive logs` & `ghive logrotate`)

```bash
# Menampilkan 50 baris log terakhir
ghive logs api-gateway

# Live stream log secara real-time
ghive logs api-gateway -f

# Hanya menampilkan log error
ghive logs api-gateway --err

# Putar log dan kompresi gzip (.gz) secara manual on-demand
ghive logrotate api-gateway
ghive logrotate all
```

### 6. Menyimpan & Memulihkan State (`save` & `resurrect`)

```bash
# Menyimpan snapshot semua proses aktif
ghive save

# Memulihkan proses setelah server reboot
ghive resurrect
```

### 7. Otomasi Startup Boot OS (`startup`)

```bash
# Membuat startup service (Systemd di Linux / Launchd di macOS)
ghive startup

# Menghapus startup service
ghive unstartup
```

### 8. Workflow Deployment Otomatis (`ghive deploy`)

```bash
# Melakukan deployment ke target lingkungan yang didefinisikan di ghive.toml
ghive deploy production
ghive deploy staging
ghive deploy local
```

### 9. Scheduled Background Jobs (`ghive cron`) ⏰

Gravhive memiliki scheduler cron bawaan tanpa membutuhkan crontab OS, lengkap dengan dukungan zona waktu global dan per-job. Anda dapat membuat cron job dengan 2 cara:

#### Cara 1: Langsung dari CLI (`ghive start --cron`)
```bash
# Menjalankan script backup setiap jam 02:00 pagi (WIB / Asia/Jakarta)
ghive start "python3 scripts/backup.py" --name "nightly-backup" --cron "0 2 * * *" --cron-timezone "Asia/Jakarta"

# Menjalankan pembersihan cache setiap 5 menit
ghive start "node clean-cache.js" --name "cache-cleaner" --cron "*/5 * * * *"

# Menjalankan health check setiap 30 detik (format 6-field dengan detik)
ghive start "curl -s http://localhost:8080/health" --name "health-ping" --cron "*/30 * * * * *"
```

#### Cara 2: Melalui File Konfigurasi (`ghive.toml`)
```toml
# Pengaturan Zona Waktu Global untuk Scheduler Cron (opsional)
[cron]
timezone = "Asia/Jakarta"            # IANA timezone (misal: "Asia/Jakarta", "America/New_York", "UTC", "local")

[app.nightly-backup]
script = "python3 scripts/backup.py"
cron = "0 2 * * *"                  # Berjalan otomatis setiap jam 02:00 pagi
cron_timezone = "Asia/Jakarta"      # Override zona waktu spesifik untuk job ini (opsional)
```

#### Mengatur & Melihat Zona Waktu Cron (`set-timezone` / `timezone`):
```bash
# Melihat zona waktu cron saat ini
ghive cron timezone
# alias cepat:
ghive cron tz

# Mengubah zona waktu default scheduler cron ke Asia/Jakarta (WIB / UTC+7)
ghive cron set-timezone Asia/Jakarta
# alias:
ghive cron set-tz Asia/Jakarta
# atau:
ghive cron timezone Asia/Jakarta

# Mengatur zona waktu mengikuti zona waktu sistem host lokal secara dinamis
ghive cron set-timezone local
```
> **Catatan:** Mengubah zona waktu secara dinamis akan langsung menghitung ulang jadwal eksekusi berikutnya (*next run*) untuk semua cron job yang menggunakan zona waktu default, dan menyimpannya secara persisten ke `~/.gravhive/cron.json`.

#### Melihat Status & Riwayat Eksekusi Cron:
```bash
# Menampilkan tabel semua scheduled jobs
ghive cron
# atau
ghive cron list
```
Output menampilkan nama job, jadwal ekspresi, zona waktu (*timezone*), perintah (*command*), jadwal eksekusi berikutnya (*next run* sesuai zona waktu), waktu eksekusi terakhir (*last run*), jumlah eksekusi (*runs count*), dan status (*Scheduled*, *Running*, *Stopped*, atau *Success* / *Failed*).

#### Menghentikan (Pause) & Melanjutkan Cron Job:
```bash
# Hentikan/pause satu cron job
ghive cron stop nightly-backup

# Atau hentikan semua cron job sekaligus
ghive cron stop all

# Lanjutkan/restart cron job yang sedang di-pause
ghive cron restart nightly-backup
ghive cron restart all
```

#### Menghapus Cron Job:
```bash
# Hapus cron job dari scheduler
ghive cron delete nightly-backup
# alias cepat:
ghive cron rm nightly-backup

# Atau hapus semua cron job
ghive cron delete all
```
> **Tips:** Anda juga dapat menggunakan perintah standar `ghive stop <nama-cron>` dan `ghive delete <nama-cron>` secara langsung. Gravhive otomatis mendeteksi jika target merupakan cron job.

#### Melihat Log Eksekusi Cron:
Log keluaran (*stdout* & *stderr*) setiap eksekusi cron otomatis tersimpan di ring buffer terpisah dan dapat dilihat dengan:
```bash
ghive logs cron:nightly-backup -n 50
```

### 10. WebAssembly Plugins (`ghive plugins`)

```bash
# Melihat daftar plugin WASM yang aktif di ~/.gravhive/plugins/
ghive plugins
```

### 11. In-Memory Secret Vault & Enkripsi Environment (`ghive secrets`)

```bash
# Menyimpan secret terenkripsi (AES-256-GCM)
ghive secrets set DATABASE_URL "postgres://app:s3cr3t@db.prod.internal:5432/main"
ghive secrets set STRIPE_KEY "sk_live_998877665544332211"

# Menampilkan daftar secret tersimpan (nilai otomatis disamarkan)
ghive secrets list

# Mengambil secret spesifik
ghive secrets get DATABASE_URL

# Mengenkripsi file plaintext .env menjadi .env.enc
ghive secrets encrypt .env --output .env.enc

# Mendekripsi .env.enc ke terminal
ghive secrets decrypt .env.enc

# Otomatis: Setiap kredensial yang terdaftar di vault akan otomatis disamarkan [REDACTED]
# pada keluaran `ghive describe` dan streaming log aplikasi.
```

### 12. Multi-Server Mesh & Centralized Management (`ghive mesh`)

```bash
# 1. Pada Server Coordinator (Master Node):
# Menjalankan Mesh Coordinator listener
ghive mesh init --port 9443 --token "cluster-secret-key"

# 2. Pada Worker Node (Server Cabang / Slave):
# Menghubungkan daemon lokal ke coordinator
ghive mesh join 192.168.1.10:9443 --token "cluster-secret-key" --name "worker-node-1"

# 3. Dari terminal mana saja (Query Cluster):
# Melihat status dan kesehatan CPU/RAM seluruh node dalam mesh
ghive mesh nodes --coordinator 192.168.1.10:9443 --token "cluster-secret-key"

# Melihat daftar agregat seluruh proses aplikasi di semua server
ghive mesh list --coordinator 192.168.1.10:9443 --token "cluster-secret-key"
```

### 13. On-Demand CPU & Memory Profiling (`ghive profile`)

```bash
# Sampling CPU aplikasi selama 10 detik dan menghasilkan file SVG Flamegraph interaktif
ghive profile api-gateway --duration 10 --output flamegraph-cpu.svg

# Profiling alokasi memori aplikasi
ghive profile bun-web --duration 5 --mode memory --output flamegraph-memory.svg

# File SVG yang dihasilkan dapat langsung dibuka di browser web apa pun (Chrome, Safari, Firefox)
# dengan fitur interaktif: zoom in/out frame alokasi, pencarian stack trace, dan hover info.
```

### 14. Pembaruan Otomatis Biner (`ghive upgrade` / `ghive update`) 🚀

Gravhive dilengkapi dengan modul *self-upgrade* bawaan yang mengunduh dan memasang versi terbaru secara otomatis langsung dari GitHub Releases:

```bash
# 1. Periksa apakah ada versi baru tanpa mengunduh
ghive upgrade --check

# 2. Upgrade Gravhive ke versi rilis terbaru
ghive upgrade
# (atau gunakan alias)
ghive update

# 3. Upgrade atau beralih ke versi spesifik (misal v0.2.0)
ghive upgrade --to 0.2.0

# 4. Paksa instalasi ulang versi saat ini
ghive upgrade --force

# Catatan: Jika Gravhive diinstal di direktori sistem (/usr/local/bin),
# jalankan dengan hak akses administrator:
# sudo ghive upgrade
```

### 15. Smart Alerting & Webhooks (`ghive webhook` / `ghive alert`) 🔔

Gravhive mendukung notifikasi langsung ke **Discord**, **Slack**, **Telegram**, dan **Generic HTTP Webhook** saat worker crash (`ProcessCrash`), memori melebihi batas (`MemoryLimitExceeded`), reload sukses (`ReloadSuccess`), atau hasil deployment (`DeployResult`).

Webhook disimpan secara persisten di `~/.gravhive/alerts.json` dan otomatis terhubung secara live (*hot reload*) ke daemon `ghived` yang sedang berjalan via IPC tanpa perlu restart daemon.

#### Menyimpan & Mengonfigurasi Webhook (`ghive webhook set`):
```bash
# 1. Simpan Discord Webhook (otomatis mendeteksi tipe Discord dari URL)
ghive webhook set https://discord.com/api/webhooks/12345/abcdef --username "Gravhive Alert"

# 2. Simpan Slack Webhook (otomatis mendeteksi tipe Slack)
ghive webhook set https://hooks.slack.com/services/T00/B00/X00 --channel "#server-alerts"

# 3. Simpan Telegram Bot (dengan chat ID)
ghive webhook set 123456:ABC-DEF --chat-id 987654321

# 4. Simpan Generic Webhook kustom dengan header HTTP
ghive webhook set https://api.perusahaan.com/alerts/webhook -H "Authorization: Bearer token123"
```

#### Menampilkan Webhook Tersimpan (`ghive webhook list`):
```bash
# Menampilkan tabel konfigurasi dengan token tersamarkan [REDACTED]
ghive webhook list
# atau alias cepat:
ghive webhook

# Tampilkan URL penuh tanpa sensor:
ghive webhook list --show-full
```

#### Menguji Konektivitas Webhook (`ghive webhook test`):
```bash
# Kirim notifikasi uji coba ke semua webhook yang terkonfigurasi
ghive webhook test

# Kirim dengan pesan kustom
ghive webhook test --message "Uji coba alert dari server produksi"
```

#### Menghapus Webhook (`ghive webhook delete`):
```bash
# Hapus webhook Discord
ghive webhook delete discord

# Hapus webhook Slack
ghive webhook delete slack

# Atau bersihkan semua webhook sekaligus
ghive webhook delete all
```

> **Tips:** Anda juga dapat menggunakan alias `ghive alert` atau `ghive alerts`.

### 16. Dynamic Worker Scaling (`ghive scale`) 📈

Tingkatkan atau turunkan jumlah worker process cluster secara on-the-fly tanpa downtime:

```bash
# Set jumlah worker aplikasi web secara absolut menjadi 4 instance
ghive scale web 4

# Tambah 2 worker baru (+2)
ghive scale web +2

# Kurangi 1 worker (-1)
ghive scale web -1
```

### 17. Docker & Container Runtime Mode (`ghive run`) 🐳

Gravhive dirancang untuk berfungsi sebagai **PID 1 Container Supervisor** yang sempurna di dalam Docker dan Kubernetes:
- Berjalan di *foreground* tanpa melepaskan proses ke background daemon.
- Meneruskan dan menangani sinyal OS `SIGTERM` dan `SIGINT` untuk graceful shutdown proses anak.
- Mengumpulkan dan melakukan *multiplexing* log stdout/stderr dari seluruh worker anak langsung ke stdout container (`docker logs`).
- Membersihkan proses anak yang mati (*zombie reaper*).

#### Menjalankan di Docker / Foreground:
```bash
# Menjalankan aplikasi langsung di foreground dengan 4 instance worker
ghive run app.js -i 4

# Menjalankan dengan file environment (.env)
ghive run app.js --env-file .env

# Menjalankan konfigurasi ekosistem ghive.toml di foreground
ghive run ghive.toml

# Atau menggunakan opsi --no-daemon pada perintah start
ghive start ghive.toml --no-daemon
```

#### Contoh `Dockerfile` Produksi:
```dockerfile
FROM rust:alpine AS builder
# ... build gravhive atau install binary pre-built ...

FROM node:20-alpine
COPY --from=builder /usr/local/bin/ghive /usr/local/bin/ghive
WORKDIR /app
COPY . .
RUN npm install --production

# Menggunakan Gravhive sebagai PID 1 supervisor container
ENTRYPOINT ["ghive", "run", "server.js", "-i", "max"]
```

### 18. Environment File Loader (`--env-file`) 🔐

Muat variabel environment dari file `.env` standar atau file `.env.enc` yang terenkripsi AES-256-GCM secara otomatis:

```bash
# Memuat variabel dari .env standar
ghive start app.js --env-file .env

# Memuat variabel dari .env.enc (otomatis didekripsi menggunakan master key vault)
ghive start app.js --env-file .env.enc

# Override nilai variabel spesifik dengan flag -e / --env
ghive start app.js --env-file .env -e PORT=8080 -e NODE_ENV=staging

# Didukung juga pada mode Docker (ghive run)
ghive run app.js --env-file .env
```

### 19. Native Prometheus Metrics Exporter (`ghive metrics`) 📊

Gravhive menyediakan HTTP Prometheus / OpenMetrics exporter bawaan berkecepatan tinggi tanpa dependensi tambahan. Endpoint ini juga menyertakan metrik yang 100% kompatibel dengan dashboard PM2 Grafana standar (`pm2_up`, `pm2_process_cpu`, `pm2_process_memory`, `pm2_process_status`, `pm2_process_restarts`).

```bash
# Menjalankan Prometheus exporter pada port default 9615 (localhost: http://127.0.0.1:9615/metrics)
ghive metrics

# Mengekspos exporter ke seluruh interface publik (0.0.0.0)
ghive metrics --host 0.0.0.0

# Menjalankan pada custom host & port
ghive metrics --host 0.0.0.0 --port 9090
```

#### Contoh Konfigurasi Scrape `prometheus.yml`:
```yaml
scrape_configs:
  - job_name: 'gravhive'
    scrape_interval: 5s
    static_configs:
      - targets: ['localhost:9615']
```

Contoh keluaran metrik (`GET /metrics`):
```text
# HELP gravhive_up GravHive daemon status (1 = reachable, 0 = unreachable)
# TYPE gravhive_up gauge
gravhive_up 1
# HELP pm2_up Process manager daemon status (PM2 compatibility)
# TYPE pm2_up gauge
pm2_up 1
# HELP gravhive_processes_total Total number of registered processes
# TYPE gravhive_processes_total gauge
gravhive_processes_total 4
# HELP gravhive_process_status Process online status (1 = online, 0 = other)
gravhive_process_status{id="0",name="web",instance="0",status="online"} 1
pm2_process_status{name="web",id="0"} 1
# HELP gravhive_process_cpu_percent Process CPU utilization percentage
gravhive_process_cpu_percent{id="0",name="web",instance="0",pid="4123"} 0.85
pm2_process_cpu{name="web",id="0"} 0.85
# HELP gravhive_process_memory_bytes Process RSS memory usage in bytes
gravhive_process_memory_bytes{id="0",name="web",instance="0",pid="4123"} 41943040
pm2_process_memory{name="web",id="0"} 41943040
```

### 20. Pre-Flight Ecosystem Validator (`ghive validate` / `ghive check`) 🔍

Cegah kegagalan deploy di lingkungan produksi dengan linter konfigurasi sebelum menjalankan `ghive start`. Perintah ini memvalidasi:
- Sintaks TOML dan JSON pada file ekosistem.
- Keberadaan file target skrip atau biner yang dikonfigurasi.
- Ketersediaan interpreter kustom di `$PATH` sistem.
- Sintaks ekspresi cron standar 5-field atau 6-field.
- Format batas memori (`max_memory_restart`).
- Deteksi tabrakan (*collision*) port TCP antara aplikasi.
- Format dan skema URL health check readiness probe.

```bash
# Validasi file ekosistem ghive.toml (atau alias ghive check)
ghive validate ghive.toml
ghive check ghive.toml

# Validasi file ekosistem format JSON (PM2 migration)
ghive validate ecosystem.config.json
```

Contoh output validasi:
```text
Validating configuration file: "ghive.toml"
  [OK] Valid syntax and ecosystem structure
  [OK] App 'api-gateway': script 'dist/index.js' found (interpreter: 'node')
  [OK] App 'worker': cron expression '0 2 * * *' is valid
  [OK] No port collisions detected across apps
  [OK] Health check URL 'http://127.0.0.1:3000/healthz' is valid

Validation successful: 0 errors, 0 warnings. Configuration is ready for production.
```

### 21. System & Runtime Environment Diagnostics (`ghive doctor`) 🩺

Jalankan audit diagnostik menyeluruh untuk memeriksa kesehatan host, socket IPC, dependensi runtime eksternal, dan alokasi resource sebelum atau sesudah menjalankan proses produksi:

```bash
ghive doctor
```

Output komprehensif mencakup:
- **Daemon & IPC:** Ketersediaan socket UNIX / IPC, status komunikasi `ghived`, dan versi daemon.
- **Security & Secret Vault:** Keberadaan dan integritas master encryption key AES-256 (`~/.gravhive/vault.key`).
- **Alerting & Webhooks:** Status dan jumlah webhook terkonfigurasi (Discord, Slack, Telegram, Custom HTTP).
- **Host Hardware Resources:** Jumlah core CPU, total kapasitas RAM, dan pemakaian memori sistem.
- **OS Limits & Kernel Isolation:** Batas limit file descriptor (`ulimit -n`) via `getrlimit`, dan status ketersediaan Linux **cgroups v2** untuk sandbox memori/CPU.
- **Installed Runtimes:** Deteksi otomatis versi `node`, `bun`, `deno`, `python3`, `cargo`, `docker`, dan `php`.
- **Log Storage Capacity:** Direktori log `~/.gravhive/logs/` dan kapasitas penyimpanan disk yang tersisa.
- **Prometheus Metrics Port:** Ketersediaan port TCP default (9615).

### 22. Lifecycle & Log Storage Garbage Collection (`ghive prune`) 🧹

Kelola efisiensi resource memori daemon dan ruang penyimpanan disk dengan membersihkan worker yang sudah tidak aktif serta file log orphan / usang:

```bash
# 1. Prune simulasi (Dry Run): Tampilkan apa yang akan dihapus tanpa modifikasi
ghive prune --dry-run

# 2. Prune seluruh worker non-aktif (stopped / errored) dan hapus file log usang
ghive prune

# 3. Prune tanpa konfirmasi interaktif (cocok untuk cron job / CI/CD pipeline)
ghive prune --force
# atau
ghive prune -f

# 4. Hanya bersihkan file log usang/orphan tanpa menghapus proses dari daftar
ghive prune --only-logs
```

Contoh keluaran `ghive prune`:
```text
Simulating prune operation (dry-run):
  - Stopped / Errored processes to remove (2):
    * worker-job (ID: 3, status: stopped)
    * broken-script (ID: 7, status: errored)
  - Log files to delete (2):
    * /Users/user/.gravhive/logs/broken-script-out.log (12.4 KB)
    * /Users/user/.gravhive/logs/broken-script-err.log (4.1 KB)
Dry-run completed: 2 processes would be removed, 2 log files (16.5 KB) would be deleted.
```

### 23. Event Timeline & Audit Journal (`ghive events` / `ghive history`) 📜

Gravhive mencatat linimasa peristiwa siklus hidup worker secara tersentralisasi di dalam circular buffer ring di memori daemon (default 500 entri terbaru). Setiap event dilengkapi timestamp RFC 3339 presisi, nama aplikasi, worker ID, PID, tipe event, dan pesan detail.

```bash
# Tampilkan 30 event siklus hidup terbaru untuk semua aplikasi
ghive events

# Tampilkan event dengan batas jumlah kustom
ghive events --limit 50
# atau
ghive history -n 50

# Filter event untuk aplikasi spesifik (nama atau worker ID)
ghive events api-gateway
ghive events 0

# Ekspor audit event dalam format JSON untuk analisis SIEM atau log monitoring
ghive events --json
```

**Tipe Event yang Dicatat:**
- `spawned`: Worker baru berhasil di-spawn dengan PID OS tertentu.
- `exited`: Worker berhenti secara graceful dengan kode status normal (`code: 0`).
- `crashed`: Worker mengalami abnormal termination (`exit code != 0` atau signal terminasi).
- `restarted`: Pemulihan otomatis via crash watchdog atau pemanggilan restart manual.
- `reloaded`: Zero-downtime rolling reload instance worker baru menggantikan worker lama.
- `scaled`: Penyesuaian jumlah worker cluster (`ghive scale`).
- `logs_rotated`: Rotasi otomatis atau manual file log stdout/stderr.

---

## ⚙️ Format File Konfigurasi Lengkap (`ghive.toml`)

Buat template konfigurasi otomatis dengan perintah:

```bash
ghive ecosystem toml
```

Contoh konfigurasi `ghive.toml`:

```toml
# --- Standar Web Application ---
[app.web-service]
script = "app.js"
instances = "max"
exec_mode = "cluster"
autorestart = true
max_memory_restart = "300MB"
watch = false
env = { NODE_ENV = "production", PORT = "8080" }

# Rotasi Log Otomatis & Kompresi
[app.web-service.log]
out = "logs/web-out.log"
err = "logs/web-err.log"
rotate_max_size = "50MB"               # Rotasi otomatis bila log mencapai 50 MB
rotate_keep = 5                        # Simpan 5 file arsip terkompresi
compress = true                        # Kompresi gzip (.gz) otomatis di background

# Health Check & Readiness Probe (Zero-Downtime Reload Verification)
[app.web-service.health_check]
http_path = "http://127.0.0.1:{port}/healthz"
timeout_secs = 3
max_retries = 3
interval_secs = 5

# Linux cgroups v2 resource hard-limits
# cgroup_memory_max = "500MB"
# cgroup_cpu_max = "50000 100000"   # Kuota 50% CPU
# cgroup_pids_max = 100

# --- Background Worker ---
[app.worker-service]
script = "./target/release/worker"
exec_mode = "fork"
instances = 1
autorestart = true
restart_delay = "500ms"
max_restarts = 10

# --- Scheduled Cron Job ---
[app.nightly-backup]
script = "python3 scripts/backup.py"
cron = "0 2 * * *"                  # Berjalan otomatis setiap jam 02:00 pagi

# --- Socket Activation & Scale-to-Zero ---
[app.staging-service]
script = "bun run server.ts"
listen_port = 8080                  # Gravhive membuka port ini
target_port = 18080                 # Worker berjalan di port internal ini
idle_timeout_secs = 300             # Berhenti otomatis setelah 5 menit tidak ada traffic

# --- Smart Alerting & Webhooks ---
# [alerts.discord]
# webhook_url = "https://discord.com/api/webhooks/..."

# [alerts.telegram]
# bot_token = "123456:ABC-DEF..."
# chat_id = "12345678"

# --- Deployment Pipeline ---
[deploy.production]
user = "deploy"
host = "192.168.1.50"               # Gunakan "local" atau IP/hostname remote
repo = "git@github.com:user/app.git"
path = "/var/www/my-app"
ref = "origin/main"
pre_deploy = "echo 'Starting deployment...'"
post_deploy = "npm install && npm run build && ghive reload all"
```

---

## 🛡️ Lisensi

Didistribusikan di bawah lisensi MIT / Apache-2.0.
