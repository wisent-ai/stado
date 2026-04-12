package model

import (
	"encoding/json"
	"time"

	"github.com/google/uuid"
)

type InstanceStatus string

const (
	InstanceStatusCreating   InstanceStatus = "creating"
	InstanceStatusRunning    InstanceStatus = "running"
	InstanceStatusStopping   InstanceStatus = "stopping"
	InstanceStatusStopped    InstanceStatus = "stopped"
	InstanceStatusDestroying InstanceStatus = "destroying"
	InstanceStatusDestroyed  InstanceStatus = "destroyed"
	InstanceStatusError      InstanceStatus = "error"
)

type Instance struct {
	ID                uuid.UUID       `json:"id" db:"id"`
	UserID            uuid.UUID       `json:"user_id" db:"user_id"`
	MachineID         uuid.UUID       `json:"machine_id" db:"machine_id"`
	HostID            uuid.UUID       `json:"host_id" db:"host_id"`
	Status            InstanceStatus  `json:"status" db:"status"`
	DockerImage       string          `json:"docker_image" db:"docker_image"`
	DockerEnv         json.RawMessage `json:"docker_env" db:"docker_env"`
	DockerPorts       json.RawMessage `json:"docker_ports" db:"docker_ports"`
	DockerCmd         *string         `json:"docker_cmd,omitempty" db:"docker_cmd"`
	DockerArgs        json.RawMessage `json:"docker_args" db:"docker_args"`
	DiskGB            int             `json:"disk_gb" db:"disk_gb"`
	SSHHost           string          `json:"ssh_host,omitempty" db:"ssh_host"`
	SSHPort           int             `json:"ssh_port,omitempty" db:"ssh_port"`
	SSHPublicKey      string          `json:"ssh_public_key,omitempty" db:"ssh_public_key"`
	JupyterURL        string          `json:"jupyter_url,omitempty" db:"jupyter_url"`
	JupyterToken      string          `json:"jupyter_token,omitempty" db:"jupyter_token"`
	PricePerHourCents int             `json:"price_per_hour_cents" db:"price_per_hour_cents"`
	TotalCostCents    int64           `json:"total_cost_cents" db:"total_cost_cents"`
	StartedAt         *time.Time      `json:"started_at,omitempty" db:"started_at"`
	StoppedAt         *time.Time      `json:"stopped_at,omitempty" db:"stopped_at"`
	DestroyedAt       *time.Time      `json:"destroyed_at,omitempty" db:"destroyed_at"`
	LastBilledAt      *time.Time      `json:"last_billed_at,omitempty" db:"last_billed_at"`
	GPUUtilization    float32         `json:"gpu_utilization" db:"gpu_utilization"`
	GPUMemoryUsedMB   int             `json:"gpu_memory_used_mb" db:"gpu_memory_used_mb"`
	CPUUtilization    float32         `json:"cpu_utilization" db:"cpu_utilization"`
	RAMUsedMB         int             `json:"ram_used_mb" db:"ram_used_mb"`
	Label             string          `json:"label,omitempty" db:"label"`
	CreatedAt         time.Time       `json:"created_at" db:"created_at"`
	UpdatedAt         time.Time       `json:"updated_at" db:"updated_at"`
	Machine           *Machine        `json:"machine,omitempty" db:"-"`
}

type CreateInstanceParams struct {
	MachineID    uuid.UUID          `json:"machine_id" validate:"required"`
	DockerImage  string             `json:"docker_image" validate:"required"`
	DockerEnv    map[string]string  `json:"docker_env,omitempty"`
	DockerPorts  map[string]string  `json:"docker_ports,omitempty"`
	DockerCmd    *string            `json:"docker_cmd,omitempty"`
	DiskGB       int                `json:"disk_gb"`
	SSHPublicKey string             `json:"ssh_public_key" validate:"required"`
	Label        string             `json:"label,omitempty"`
}

type InstanceEvent struct {
	ID         int64           `json:"id" db:"id"`
	InstanceID uuid.UUID       `json:"instance_id" db:"instance_id"`
	EventType  string          `json:"event_type" db:"event_type"`
	Details    json.RawMessage `json:"details" db:"details"`
	CreatedAt  time.Time       `json:"created_at" db:"created_at"`
}
