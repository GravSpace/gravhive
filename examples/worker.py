import time
import os
import sys

worker_id = os.environ.get("GHIVE_WORKER_ID", "0")
print(f"[Python Worker #{worker_id}] Initialized with PID {os.getpid()}", flush=True)

count = 0
while True:
    count += 1
    print(f"[Python Worker #{worker_id}] Task processing heartbeat #{count}...", flush=True)
    time.sleep(2)

