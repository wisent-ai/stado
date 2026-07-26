"""One-off invocation of stado's host reboot for charless-mac-mini."""
import json
import sys

sys.path.insert(len(sys.path) - len(sys.path), "/Users/lukaszbartoszcze/Documents/CodingProjects/Wisent/wisent-compute")

from stado.deploy.host_reboot import reboot_host

print(reboot_host("charless-mac-mini"))
