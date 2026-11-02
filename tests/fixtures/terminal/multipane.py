"""Finite output and repeated input acknowledgements for multi-pane UI benchmarks."""
import os
import sys
import threading
import tty


def write_all(data):
    while data:
        written = os.write(1, data)
        data = data[written:]


tty.setraw(0)
count = int(sys.argv[1])
ack_path = sys.argv[2]
record = "Compiling module: \x1b[32mcomplete\x1b[0m 界 café e\u0301 🚀\r\n".encode()
block = record * (16384 // len(record))
write_all(b"\r\nMULTI_READY\r\n")
assert os.read(0, 1) == b"s"


def acknowledge():
    count = 0
    while True:
        byte = os.read(0, 1)
        if byte != b"p":
            return
        count += 1
        temporary = ack_path + ".tmp"
        with open(temporary, "w") as output:
            output.write(str(count))
        os.replace(temporary, ack_path)


reader = threading.Thread(target=acknowledge)
reader.start()
remaining = count
while remaining:
    data = block if remaining >= len(block) else record * (remaining // len(record)) + b" " * (remaining % len(record))
    write_all(data)
    remaining -= len(data)
write_all(b"\r\nMULTI_DONE\r\n")
reader.join()
