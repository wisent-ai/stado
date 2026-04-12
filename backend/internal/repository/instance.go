package repository

import (
	"context"
	"encoding/json"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
)

type InstanceRepo struct {
	db *pgxpool.Pool
}

func NewInstanceRepo(db *pgxpool.Pool) *InstanceRepo {
	return &InstanceRepo{db: db}
}

func (r *InstanceRepo) Create(ctx context.Context, params model.CreateInstanceParams, userID, hostID uuid.UUID, priceCents int) (*model.Instance, error) {
	envJSON, _ := json.Marshal(params.DockerEnv)
	portsJSON, _ := json.Marshal(params.DockerPorts)

	inst := &model.Instance{}
	err := r.db.QueryRow(ctx,
		`INSERT INTO instances (user_id, machine_id, host_id, docker_image, docker_env, docker_ports,
			docker_cmd, disk_gb, ssh_public_key, price_per_hour_cents, label)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
		 RETURNING id, user_id, machine_id, host_id, status, docker_image, disk_gb,
				  price_per_hour_cents, label, created_at, updated_at`,
		userID, params.MachineID, hostID, params.DockerImage, envJSON, portsJSON,
		params.DockerCmd, params.DiskGB, params.SSHPublicKey, priceCents, params.Label).Scan(
		&inst.ID, &inst.UserID, &inst.MachineID, &inst.HostID, &inst.Status,
		&inst.DockerImage, &inst.DiskGB, &inst.PricePerHourCents, &inst.Label,
		&inst.CreatedAt, &inst.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}
	return inst, nil
}

func (r *InstanceRepo) GetByID(ctx context.Context, id uuid.UUID) (*model.Instance, error) {
	inst := &model.Instance{}
	err := r.db.QueryRow(ctx,
		`SELECT id, user_id, machine_id, host_id, status, docker_image, docker_env, docker_ports,
				docker_cmd, disk_gb, ssh_host, ssh_port, ssh_public_key, jupyter_url, jupyter_token,
				price_per_hour_cents, total_cost_cents, started_at, stopped_at, destroyed_at,
				last_billed_at, gpu_utilization, gpu_memory_used_mb, cpu_utilization, ram_used_mb,
				label, created_at, updated_at
		 FROM instances WHERE id = $1`, id).Scan(
		&inst.ID, &inst.UserID, &inst.MachineID, &inst.HostID, &inst.Status,
		&inst.DockerImage, &inst.DockerEnv, &inst.DockerPorts, &inst.DockerCmd,
		&inst.DiskGB, &inst.SSHHost, &inst.SSHPort, &inst.SSHPublicKey,
		&inst.JupyterURL, &inst.JupyterToken, &inst.PricePerHourCents,
		&inst.TotalCostCents, &inst.StartedAt, &inst.StoppedAt, &inst.DestroyedAt,
		&inst.LastBilledAt, &inst.GPUUtilization, &inst.GPUMemoryUsedMB,
		&inst.CPUUtilization, &inst.RAMUsedMB, &inst.Label, &inst.CreatedAt, &inst.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}
	return inst, nil
}

func (r *InstanceRepo) ListByUser(ctx context.Context, userID uuid.UUID) ([]model.Instance, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, user_id, machine_id, host_id, status, docker_image, disk_gb,
				ssh_host, ssh_port, price_per_hour_cents, total_cost_cents,
				started_at, stopped_at, label, created_at
		 FROM instances WHERE user_id = $1 ORDER BY created_at DESC`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var instances []model.Instance
	for rows.Next() {
		var inst model.Instance
		if err := rows.Scan(
			&inst.ID, &inst.UserID, &inst.MachineID, &inst.HostID, &inst.Status,
			&inst.DockerImage, &inst.DiskGB, &inst.SSHHost, &inst.SSHPort,
			&inst.PricePerHourCents, &inst.TotalCostCents,
			&inst.StartedAt, &inst.StoppedAt, &inst.Label, &inst.CreatedAt,
		); err != nil {
			return nil, err
		}
		instances = append(instances, inst)
	}
	return instances, nil
}

func (r *InstanceRepo) UpdateStatus(ctx context.Context, id uuid.UUID, status model.InstanceStatus) error {
	_, err := r.db.Exec(ctx, `UPDATE instances SET status = $2 WHERE id = $1`, id, status)
	return err
}

func (r *InstanceRepo) SetRunning(ctx context.Context, id uuid.UUID, sshHost string, sshPort int) error {
	_, err := r.db.Exec(ctx,
		`UPDATE instances SET status = 'running', started_at = now(), last_billed_at = now(),
			ssh_host = $2, ssh_port = $3 WHERE id = $1`,
		id, sshHost, sshPort)
	return err
}

func (r *InstanceRepo) SetDestroyed(ctx context.Context, id uuid.UUID) error {
	_, err := r.db.Exec(ctx,
		`UPDATE instances SET status = 'destroyed', destroyed_at = now() WHERE id = $1`, id)
	return err
}

func (r *InstanceRepo) ListRunning(ctx context.Context) ([]model.Instance, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, user_id, machine_id, host_id, price_per_hour_cents, last_billed_at, started_at
		 FROM instances WHERE status = 'running'`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var instances []model.Instance
	for rows.Next() {
		var inst model.Instance
		if err := rows.Scan(
			&inst.ID, &inst.UserID, &inst.MachineID, &inst.HostID,
			&inst.PricePerHourCents, &inst.LastBilledAt, &inst.StartedAt,
		); err != nil {
			return nil, err
		}
		instances = append(instances, inst)
	}
	return instances, nil
}

func (r *InstanceRepo) RecordBilling(ctx context.Context, id uuid.UUID, costCents int64) error {
	_, err := r.db.Exec(ctx,
		`UPDATE instances SET last_billed_at = now(), total_cost_cents = total_cost_cents + $2 WHERE id = $1`,
		id, costCents)
	return err
}

func (r *InstanceRepo) InsertEvent(ctx context.Context, instanceID uuid.UUID, eventType string, details interface{}) error {
	detailsJSON, _ := json.Marshal(details)
	_, err := r.db.Exec(ctx,
		`INSERT INTO instance_events (instance_id, event_type, details) VALUES ($1, $2, $3)`,
		instanceID, eventType, detailsJSON)
	return err
}
