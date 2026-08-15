#!/bin/sh
set -eu

python3 - <<'PY'
import socket
import time

mac = bytes.fromhex("30c59923ef02")
packet = b"\xff" * 6 + mac * 16
addresses = (("255.255.255.255", 9), ("10.0.0.255", 9))

with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    for _ in range(5):
        for address in addresses:
            sock.sendto(packet, address)
        time.sleep(0.2)

print("sent Wake-on-LAN packet for ubuntu-server-rtx-pro-6000")
PY
