package main

import (
	"flag"
	"os"

	"github.com/rs/zerolog"
	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/agent/internal/command"
	"github.com/wisent-ai/compute.wisent.com/agent/internal/config"
	containermgr "github.com/wisent-ai/compute.wisent.com/agent/internal/container"
	"github.com/wisent-ai/compute.wisent.com/agent/internal/heartbeat"
)

func main() {
	zerolog.TimeFieldFormat = zerolog.TimeFormatUnix
	log.Logger = log.Output(zerolog.ConsoleWriter{Out: os.Stderr})

	configPath := flag.String("config", "/etc/wisent-agent/config.yaml", "path to config file")
	flag.Parse()

	cfg, err := config.Load(*configPath)
	if err != nil {
		log.Fatal().Err(err).Msg("failed to load config")
	}

	if cfg.ServerURL == "" || cfg.MachineID == "" || cfg.AgentToken == "" {
		log.Fatal().Msg("server_url, machine_id, and agent_token are required")
	}

	log.Info().Str("server", cfg.ServerURL).Str("machine", cfg.MachineID).Msg("starting wisent agent")

	mgr, err := containermgr.NewManager()
	if err != nil {
		log.Fatal().Err(err).Msg("failed to initialize container manager")
	}

	hbLoop, err := heartbeat.NewLoop(cfg)
	if err != nil {
		log.Fatal().Err(err).Msg("failed to initialize heartbeat")
	}

	exec := command.NewExecutor(cfg, mgr)

	go hbLoop.Run()
	go exec.PollLoop()

	log.Info().Msg("agent running")
	select {}
}
