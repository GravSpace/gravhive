# Gravhive (`ghive`) 🚀

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
- **Auto-Restart & Crash Loop Protection:** Menangkap exit code $\ne 0$ dan menerapkan *Exponential Backoff with Jitter* untuk mencegah CPU thrashing.
- **Cluster Mode & Zero-Downtime Hot Reload:** Menjalankan worker di beberapa core CPU sekaligus (`-i max`) dan melakukan rolling update tanpa koneksi terputus (`ghive reload`).
- **Max Memory Watchdog:** Memantau konsumsi RAM setiap worker secara real-time via `sysinfo`. Merestart otomatis saat ambang batas memori terlampaui (`--max-memory-restart`).
- **Watch & Reload:** Memantau perubahan file secara event-driven via kernel inotify/kqueue (`--watch`).
- **Log Management & Streaming:** Non-blocking async ring buffer di memori serta file log terisolasi (`ghive logs <app> -f`).
- **Full-Screen TUI Dashboard (`ghive monit`):** Antarmuka terminal interaktif berbasis `ratatui` untuk memantau metrik CPU, RAM, status, dan logs secara langsung.
- **State Persistence & Startup Integration:** Snapshot konfigurasi (`ghive save` & `ghive resurrect`) dan generator service boot OS (`ghive startup` untuk Systemd & macOS Launchd).

---

## 🛠️ Instalasi & Kompilasi

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

# Menghapus proses dari pengawasan
ghive delete api-gateway
ghive delete all
```

### 4. Interactive TUI Monitor (`ghive monit`)
```bash
ghive monit
```
*Gunakan tombol `[↑/↓]` untuk navigasi proses, `[R]` restart, `[S]` stop, `[D]` delete, dan `[Q]` untuk keluar.*

### 5. Melihat Log Aplikasi (`ghive logs`)
```bash
# Menampilkan 50 baris log terakhir
ghive logs api-gateway

# Live stream log secara real-time
ghive logs api-gateway -f

# Hanya menampilkan log error
ghive logs api-gateway --err
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

---

## ⚙️ Format File Konfigurasi (`ghive.toml`)

Buat template konfigurasi otomatis dengan perintah:
```bash
ghive ecosystem toml
```

Contoh konfigurasi `ghive.toml`:
```toml
[app.web-service]
script = "app.js"
instances = "max"
exec_mode = "cluster"
autorestart = true
max_memory_restart = "300MB"
watch = false
env = { NODE_ENV = "production", PORT = "8080" }

[app.worker-service]
script = "./target/release/worker"
exec_mode = "fork"
instances = 1
autorestart = true
restart_delay = "500ms"
max_restarts = 10
```

---

## 🛡️ Lisensi
Didistribusikan di bawah lisensi MIT / Apache-2.0.

