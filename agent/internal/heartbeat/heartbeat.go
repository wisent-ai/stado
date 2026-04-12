package heartbeat

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"time"

	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/agent/internal/collector"
	"github.com/wisent-ai/compute.wisent.com/agent/internal/config"
)

const agentVersion = "0.1.0"

type Payload struct {
	MachineID    string               `json:"machine_id"`
	AgentVersion string               `json:"agent_version"`
	Timestamp    string               `json:"timestamp"`
	System       *collector.SystemInfo `json:"system"`
	GPUs         []collector.GPUInfo   `json:"gpus"`
	Docker       *collector.DockerInfo `json:"docker"`
}

type Loop struct {
	cfg      *config.Config
	interval time.Duration
}

func NewLoop(cfg *config.Config) (*Loop, error) {
	d, err := time.ParseDuration(cfg.HeartbeatInterval)
	if err != nil {
		d = 30 * time.Second
	}
	return &Loop{cfg: cfg, interval: d}, nil
}

func (l *Loop) Run() {
	ticker := time.NewTicker(l.interval)
	defer ticker.Stop()

	l.beat()
	for range ticker.C {
		l.beat()
	}
}

func (l *Loop) beat() {
	sys, _ := collector.CollectSystem()
	gpus, _ := collector.CollectGPUs()
	dock, _ := collector.CollectDocker()

	payload := Payload{
		MachineID:    l.cfg.MachineID,
		AgentVersion: agentVersion,
		Timestamp:    time.Now().UTC().Format(time.RFC3339),
		System:       sys,
		GPUs:         gpus,
		Docker:       dock,
	}

	body, err := json.Marshal(payload)
	if err != nil {
		log.Error().Err(err).Msg("failed to marshal heartbeat")
		return
	}

	url := fmt.Sprintf("%s/api/v1/agent/heartbeat", l.cfg.ServerURL)
	req, err := http.NewRequest("POST", url, bytes.NewReader(body))
	if err != nil {
		log.Error().Err(err).Msg("failed to create heartbeat request")
		return
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+l.cfg.AgentToken)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		log.Warn().Err(err).Msg("heartbeat request failed")
		return
	}
	resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		log.Warn().Int("status", resp.StatusCode).Msg("heartbeat non-200 response")
	}
}
