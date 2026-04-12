package container

import (
	"context"
	"fmt"
	"io"
	"math/rand"
	"strings"

	"github.com/docker/docker/api/types/container"
	"github.com/docker/docker/api/types/image"
	"github.com/docker/docker/client"
	"github.com/docker/go-connections/nat"
	"github.com/rs/zerolog/log"
)

type CreateParams struct {
	InstanceID   string            `json:"instance_id"`
	DockerImage  string            `json:"docker_image"`
	Env          map[string]string `json:"env"`
	Ports        map[string]string `json:"ports"`
	GPUIDs       []string          `json:"gpu_ids"`
	DiskGB       int               `json:"disk_gb"`
	SSHPublicKey string            `json:"ssh_public_key"`
	Cmd          *string           `json:"cmd"`
}

type CreateResult struct {
	ContainerID  string `json:"container_id"`
	SSHPort      int    `json:"ssh_port"`
	JupyterPort  int    `json:"jupyter_port"`
	JupyterToken string `json:"jupyter_token"`
}

type Manager struct {
	cli *client.Client
}

func NewManager() (*Manager, error) {
	cli, err := client.NewClientWithOpts(client.FromEnv)
	if err != nil {
		return nil, err
	}
	return &Manager{cli: cli}, nil
}

func (m *Manager) Create(ctx context.Context, params CreateParams) (*CreateResult, error) {
	log.Info().Str("image", params.DockerImage).Str("instance", params.InstanceID).Msg("pulling image")
	reader, err := m.cli.ImagePull(ctx, params.DockerImage, image.PullOptions{})
	if err != nil {
		return nil, fmt.Errorf("image pull failed: %w", err)
	}
	io.Copy(io.Discard, reader)
	reader.Close()

	sshPort := randomPort()
	jupyterPort := randomPort()

	env := []string{}
	for k, v := range params.Env {
		env = append(env, k+"="+v)
	}
	env = append(env, "SSH_PUBLIC_KEY="+params.SSHPublicKey)

	exposedPorts := nat.PortSet{
		"22/tcp":   struct{}{},
		"8888/tcp": struct{}{},
	}

	portBindings := nat.PortMap{
		"22/tcp":   {{HostPort: fmt.Sprintf("%d", sshPort)}},
		"8888/tcp": {{HostPort: fmt.Sprintf("%d", jupyterPort)}},
	}

	var cmd []string
	if params.Cmd != nil && *params.Cmd != "" {
		cmd = strings.Fields(*params.Cmd)
	}

	containerCfg := &container.Config{
		Image:        params.DockerImage,
		Env:          env,
		ExposedPorts: exposedPorts,
		Cmd:          cmd,
		Labels:       map[string]string{"wisent.instance_id": params.InstanceID},
	}

	gpuOpts := container.DeviceRequest{
		Capabilities: [][]string{{"gpu"}},
		Count:        -1,
	}
	if len(params.GPUIDs) > 0 {
		gpuOpts.Count = 0
		gpuOpts.DeviceIDs = params.GPUIDs
	}

	hostCfg := &container.HostConfig{
		PortBindings: portBindings,
		Resources: container.Resources{
			DeviceRequests: []container.DeviceRequest{gpuOpts},
		},
		RestartPolicy: container.RestartPolicy{Name: "unless-stopped"},
	}

	name := "wisent-" + params.InstanceID[:8]
	resp, err := m.cli.ContainerCreate(ctx, containerCfg, hostCfg, nil, nil, name)
	if err != nil {
		return nil, fmt.Errorf("container create failed: %w", err)
	}

	if err := m.cli.ContainerStart(ctx, resp.ID, container.StartOptions{}); err != nil {
		return nil, fmt.Errorf("container start failed: %w", err)
	}

	log.Info().Str("container", resp.ID[:12]).Int("ssh_port", sshPort).Msg("container started")

	return &CreateResult{
		ContainerID:  resp.ID,
		SSHPort:      sshPort,
		JupyterPort:  jupyterPort,
		JupyterToken: params.Env["JUPYTER_TOKEN"],
	}, nil
}

func (m *Manager) Stop(ctx context.Context, instanceID string) error {
	cid, err := m.findContainer(ctx, instanceID)
	if err != nil {
		return err
	}
	return m.cli.ContainerStop(ctx, cid, container.StopOptions{})
}

func (m *Manager) Start(ctx context.Context, instanceID string) error {
	cid, err := m.findContainer(ctx, instanceID)
	if err != nil {
		return err
	}
	return m.cli.ContainerStart(ctx, cid, container.StartOptions{})
}

func (m *Manager) Destroy(ctx context.Context, instanceID string) error {
	cid, err := m.findContainer(ctx, instanceID)
	if err != nil {
		return err
	}
	return m.cli.ContainerRemove(ctx, cid, container.RemoveOptions{Force: true})
}

func (m *Manager) findContainer(ctx context.Context, instanceID string) (string, error) {
	containers, err := m.cli.ContainerList(ctx, container.ListOptions{All: true})
	if err != nil {
		return "", err
	}
	for _, c := range containers {
		if c.Labels["wisent.instance_id"] == instanceID {
			return c.ID, nil
		}
	}
	return "", fmt.Errorf("container not found for instance %s", instanceID)
}

func randomPort() int {
	return 30000 + rand.Intn(10000)
}
