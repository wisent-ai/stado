#!/bin/bash
set -e

ARCH=$(uname -m)
case $ARCH in
    x86_64) ARCH="amd64" ;;
    aarch64) ARCH="arm64" ;;
    *) echo "Unsupported architecture: $ARCH" && exit 1 ;;
esac

VERSION="latest"
BASE_URL="https://github.com/wisent-ai/compute.wisent.com/releases/download/${VERSION}"
BINARY="wisent-agent-linux-${ARCH}"

echo "Installing Wisent Agent (${ARCH})..."

curl -sSL "${BASE_URL}/${BINARY}" -o /tmp/wisent-agent
chmod +x /tmp/wisent-agent
sudo mv /tmp/wisent-agent /usr/local/bin/wisent-agent

sudo mkdir -p /etc/wisent-agent

if [ ! -f /etc/wisent-agent/config.yaml ]; then
    read -p "Machine ID: " MACHINE_ID
    read -p "Agent Token: " AGENT_TOKEN
    read -p "Server URL [https://api.compute.wisent.com]: " SERVER_URL
    SERVER_URL=${SERVER_URL:-https://api.compute.wisent.com}

    sudo tee /etc/wisent-agent/config.yaml > /dev/null <<EOF
server_url: ${SERVER_URL}
machine_id: ${MACHINE_ID}
agent_token: ${AGENT_TOKEN}
heartbeat_interval: 30s
EOF
fi

sudo tee /etc/systemd/system/wisent-agent.service > /dev/null <<EOF
[Unit]
Description=Wisent GPU Agent
After=network.target docker.service
Requires=docker.service

[Service]
ExecStart=/usr/local/bin/wisent-agent
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable wisent-agent
sudo systemctl start wisent-agent

echo "Wisent Agent installed and running."
echo "Check status: sudo systemctl status wisent-agent"
echo "View logs: sudo journalctl -u wisent-agent -f"
