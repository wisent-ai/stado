#!/usr/bin/env bash
# Report whether this host publishes the inference endpoint it declares.
#
# `inference blockers` can show a live VLLM::EngineCore while the registry's
# endpoint answers nothing, and from the calling side those two facts arrive as
# one: "provider request failed". The difference between a dead engine, an
# engine bound to loopback, and a container with no published port decides
# whether the repair is a restart or a port mapping, and only this host can say
# which it is.
#
# Read-only. Prints listening sockets and container port mappings, no payloads.
set -u

echo "=== listening TCP sockets ==="
if command -v ss >/dev/null; then
  ss -lntp 2>/dev/null | /usr/bin/awk 'NR>1 {print $4"  "$6}' | sort -u | head -20
else
  echo "(ss unavailable)"
fi

echo
echo "=== containers and their published ports ==="
if command -v docker >/dev/null; then
  docker ps --format '{{.Names}}  {{.Status}}  {{.Ports}}' 2>/dev/null | head -10
else
  echo "(docker unavailable)"
fi

echo
echo "=== vllm processes ==="
/bin/ps -eo pid=,args= 2>/dev/null | /bin/grep '[v]llm' | /usr/bin/cut -c1-160 | head -4

echo
echo "=== does anything answer an OpenAI models call on loopback ==="
for port in 8000 8001 8002 8003 8080; do
  code=$(curl -s -m 4 -o /dev/null -w '%{http_code}' "http://127.0.0.1:${port}/v1/models" 2>/dev/null || echo 000)
  [ "$code" = "000" ] || echo "port ${port}: HTTP ${code}"
done
echo "(no port lines above means nothing serves an OpenAI API here)"
