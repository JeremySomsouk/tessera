"""Bounded, deterministic PTY benchmark producer. No external dependencies."""
import os
import sys
import tty
import threading
import time

tty.setraw(0)
kind, count = sys.argv[1], int(sys.argv[2])
records = {
    "build": b"Compiling crate v1.0.0: warning: unused variable at src/main.rs:42\r\n",
    "agent": "\x1b[32mtool complete\x1b[0m: 界 café e\u0301 🚀\r\n".encode(),
    "line": b"x" * 16384,
}
record = records["agent" if kind == "soak" else kind]
block = record * max(1, 16384 // len(record))
os.write(1, b"\r\nBENCH_READY\r\n")
assert os.read(0, 1) == b"s"
def acknowledge():
    assert os.read(0, 1) == b"p"
    with open(sys.argv[3], "wb") as output:
        output.write(b"p")

thread = threading.Thread(target=acknowledge)
thread.start()
remaining = count
while remaining:
    data = block if remaining >= len(block) else record * (remaining // len(record)) + b" " * (remaining % len(record))
    while data:
        written = os.write(1, data)
        remaining -= written
        data = data[written:]
    if kind == "soak":
        time.sleep(0.001)
os.write(1, b"\r\nBENCH_DONE\r\n")
thread.join()
os.read(0, 1)
