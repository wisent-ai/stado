package command

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"time"

	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/agent/internal/config"
	containermgr "github.com/wisent-ai/compute.wisent.com/agent/internal/container"
)

type Command struct {
	ID         string                    `json:"id"`
	Type       string                    `json:"type"`
	InstanceID string                    `json:"instance_id"`
	Payload    containermgr.CreateParams `json:"payload"`
}

type CommandsResponse struct {
	Commands []Command `json:"commands"`
}

type AckPayload struct {
	Success bool        `json:"success"`
	Result  interface{} `json:"result,omitempty"`
	Error   string      `json:"error,omitempty"`
}

type Executor struct {
	cfg     *config.Config
	mgr     *containermgr.Manager
}

func NewExecutor(cfg *config.Config, mgr *containermgr.Manager) *Executor {
	return &Executor{cfg: cfg, mgr: mgr}
}

func (e *Executor) PollLoop() {
	ticker := time.NewTicker(5 * time.Second)
	defer ticker.Stop()

	for range ticker.C {
		e.poll()
	}
}

func (e *Executor) poll() {
	url := fmt.Sprintf("%s/api/v1/agent/commands", e.cfg.ServerURL)
	req, _ := http.NewRequest("GET", url, nil)
	req.Header.Set("Authorization", "Bearer "+e.cfg.AgentToken)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return
	}

	var cmds CommandsResponse
	if err := json.NewDecoder(resp.Body).Decode(&cmds); err != nil {
		return
	}

	for _, cmd := range cmds.Commands {
		e.execute(cmd)
	}
}

func (e *Executor) execute(cmd Command) {
	ctx := context.Background()
	var result interface{}
	var execErr error

	log.Info().Str("type", cmd.Type).Str("instance", cmd.InstanceID).Msg("executing command")

	switch cmd.Type {
	case "CREATE_CONTAINER":
		cmd.Payload.InstanceID = cmd.InstanceID
		result, execErr = e.mgr.Create(ctx, cmd.Payload)
	case "START_CONTAINER":
		execErr = e.mgr.Start(ctx, cmd.InstanceID)
	case "STOP_CONTAINER":
		execErr = e.mgr.Stop(ctx, cmd.InstanceID)
	case "DESTROY_CONTAINER":
		execErr = e.mgr.Destroy(ctx, cmd.InstanceID)
	default:
		log.Warn().Str("type", cmd.Type).Msg("unknown command type")
		return
	}

	ack := AckPayload{Success: execErr == nil, Result: result}
	if execErr != nil {
		ack.Error = execErr.Error()
		log.Error().Err(execErr).Str("type", cmd.Type).Msg("command execution failed")
	}

	e.sendAck(cmd.ID, ack)
}

func (e *Executor) sendAck(cmdID string, ack AckPayload) {
	body, _ := json.Marshal(ack)
	url := fmt.Sprintf("%s/api/v1/agent/commands/%s/ack", e.cfg.ServerURL, cmdID)
	req, _ := http.NewRequest("POST", url, bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+e.cfg.AgentToken)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		log.Error().Err(err).Msg("failed to send ack")
		return
	}
	resp.Body.Close()
}
