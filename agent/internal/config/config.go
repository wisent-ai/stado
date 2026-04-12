package config

import (
	"os"

	"gopkg.in/yaml.v3"
)

type Config struct {
	ServerURL         string `yaml:"server_url" json:"server_url"`
	MachineID         string `yaml:"machine_id" json:"machine_id"`
	AgentToken        string `yaml:"agent_token" json:"agent_token"`
	HeartbeatInterval string `yaml:"heartbeat_interval" json:"heartbeat_interval"`
}

func Load(path string) (*Config, error) {
	cfg := &Config{
		HeartbeatInterval: "30s",
	}

	if path != "" {
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, err
		}
		if err := yaml.Unmarshal(data, cfg); err != nil {
			return nil, err
		}
	}

	if v := os.Getenv("WISENT_SERVER_URL"); v != "" {
		cfg.ServerURL = v
	}
	if v := os.Getenv("WISENT_MACHINE_ID"); v != "" {
		cfg.MachineID = v
	}
	if v := os.Getenv("WISENT_AGENT_TOKEN"); v != "" {
		cfg.AgentToken = v
	}

	return cfg, nil
}
