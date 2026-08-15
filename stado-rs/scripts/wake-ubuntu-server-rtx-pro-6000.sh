#!/bin/sh
set -eu

python3 - <<'PY'
import ipaddress
import socket
import time

mac = bytes.fromhex("30c59923ef02")
packet = b"\xff" * 6 + mac * 16
with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
    route.connect(("1.1.1.1", 53))
    local_address = route.getsockname()[0]
subnet_broadcast = str(
    ipaddress.ip_network(f"{local_address}/24", strict=False).broadcast_address
)
addresses = {
    ("255.255.255.255", 7),
    ("255.255.255.255", 9),
    (subnet_broadcast, 7),
    (subnet_broadcast, 9),
}

with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    for _ in range(10):
        for address in addresses:
            sock.sendto(packet, address)
        time.sleep(0.2)

print(
    "sent Wake-on-LAN packet for ubuntu-server-rtx-pro-6000 "
    f"from {local_address} through {subnet_broadcast}"
)
PY
