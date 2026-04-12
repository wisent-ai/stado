package collector

import (
	"context"

	"github.com/docker/docker/api/types"
	"github.com/docker/docker/api/types/container"
	"github.com/docker/docker/client"
)

type DockerInfo struct {
	Version           string `json:"version"`
	RunningContainers int    `json:"running_containers"`
	IsRunning         bool   `json:"is_running"`
}

func CollectDocker() (*DockerInfo, error) {
	cli, err := client.NewClientWithOpts(client.FromEnv)
	if err != nil {
		return &DockerInfo{IsRunning: false}, nil
	}
	defer cli.Close()

	ctx := context.Background()

	info, err := cli.ServerVersion(ctx)
	if err != nil {
		return &DockerInfo{IsRunning: false}, nil
	}

	containers, err := cli.ContainerList(ctx, container.ListOptions{All: false})
	if err != nil {
		return &DockerInfo{Version: info.Version, IsRunning: true}, nil
	}

	return &DockerInfo{
		Version:           info.Version,
		RunningContainers: len(containers),
		IsRunning:         true,
	}, nil

	_ = types.ContainerJSON{}
}
