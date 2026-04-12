package repository

import (
	"context"
	"fmt"
	"strings"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
)

type MachineRepo struct {
	db *pgxpool.Pool
}

func NewMachineRepo(db *pgxpool.Pool) *MachineRepo {
	return &MachineRepo{db: db}
}

func (r *MachineRepo) Create(ctx context.Context, hostID uuid.UUID, params model.RegisterMachineParams, tokenHash string) (*model.Machine, error) {
	m := &model.Machine{}
	err := r.db.QueryRow(ctx,
		`INSERT INTO machines (host_id, label, price_per_hour_cents, min_rental_hours, max_rental_hours, country, region, agent_token_hash)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
		 RETURNING id, host_id, label, status, gpu_count, price_per_hour_cents, min_rental_hours,
				  is_available, uptime_percentage, total_rentals, created_at, updated_at`,
		hostID, params.Label, params.PricePerHourCents, params.MinRentalHours,
		params.MaxRentalHours, params.Country, params.Region, tokenHash).Scan(
		&m.ID, &m.HostID, &m.Label, &m.Status, &m.GPUCount, &m.PricePerHourCents,
		&m.MinRentalHours, &m.IsAvailable, &m.UptimePercentage, &m.TotalRentals,
		&m.CreatedAt, &m.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}
	return m, nil
}

func (r *MachineRepo) GetByID(ctx context.Context, id uuid.UUID) (*model.Machine, error) {
	m := &model.Machine{}
	err := r.db.QueryRow(ctx,
		`SELECT m.id, m.host_id, m.hostname, m.label, m.status, m.gpu_model_id, m.gpu_count,
				m.gpu_ram_gb, m.cpu_model, m.cpu_cores, m.ram_gb, m.disk_gb, m.disk_type,
				m.upload_mbps, m.download_mbps, m.country, m.region, m.cuda_version,
				m.docker_version, m.os_version, m.driver_version, m.price_per_hour_cents,
				m.min_rental_hours, m.max_rental_hours, m.agent_version, m.last_heartbeat,
				m.uptime_percentage, m.total_rentals, m.avg_rating, m.is_available,
				m.current_instance_id, m.created_at, m.updated_at
		 FROM machines m WHERE m.id = $1`, id).Scan(
		&m.ID, &m.HostID, &m.Hostname, &m.Label, &m.Status, &m.GPUModelID, &m.GPUCount,
		&m.GPURAMGB, &m.CPUModel, &m.CPUCores, &m.RAMGB, &m.DiskGB, &m.DiskType,
		&m.UploadMbps, &m.DownloadMbps, &m.Country, &m.Region, &m.CUDAVersion,
		&m.DockerVersion, &m.OSVersion, &m.DriverVersion, &m.PricePerHourCents,
		&m.MinRentalHours, &m.MaxRentalHours, &m.AgentVersion, &m.LastHeartbeat,
		&m.UptimePercentage, &m.TotalRentals, &m.AvgRating, &m.IsAvailable,
		&m.CurrentInstanceID, &m.CreatedAt, &m.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}
	return m, nil
}

func (r *MachineRepo) ListByHost(ctx context.Context, hostID uuid.UUID) ([]model.Machine, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, host_id, hostname, label, status, gpu_model_id, gpu_count, gpu_ram_gb,
				cpu_model, cpu_cores, ram_gb, disk_gb, price_per_hour_cents,
				is_available, last_heartbeat, uptime_percentage, total_rentals, created_at
		 FROM machines WHERE host_id = $1 ORDER BY created_at DESC`, hostID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var machines []model.Machine
	for rows.Next() {
		var m model.Machine
		if err := rows.Scan(
			&m.ID, &m.HostID, &m.Hostname, &m.Label, &m.Status, &m.GPUModelID, &m.GPUCount,
			&m.GPURAMGB, &m.CPUModel, &m.CPUCores, &m.RAMGB, &m.DiskGB,
			&m.PricePerHourCents, &m.IsAvailable, &m.LastHeartbeat,
			&m.UptimePercentage, &m.TotalRentals, &m.CreatedAt,
		); err != nil {
			return nil, err
		}
		machines = append(machines, m)
	}
	return machines, nil
}

type ListOffersFilter struct {
	GPUModelIDs []int
	MinVRAMGB   int
	MaxPriceCents int
	MinPriceCents int
	Country     string
	SortBy      string
	SortDir     string
	Limit       int
	Offset      int
}

func (r *MachineRepo) ListOffers(ctx context.Context, f ListOffersFilter) ([]model.Machine, int, error) {
	where := []string{"m.is_available = true", "m.status = 'online'"}
	args := []interface{}{}
	argIdx := 1

	if len(f.GPUModelIDs) > 0 {
		placeholders := make([]string, len(f.GPUModelIDs))
		for i, id := range f.GPUModelIDs {
			placeholders[i] = fmt.Sprintf("$%d", argIdx)
			args = append(args, id)
			argIdx++
		}
		where = append(where, fmt.Sprintf("m.gpu_model_id IN (%s)", strings.Join(placeholders, ",")))
	}
	if f.MinVRAMGB > 0 {
		where = append(where, fmt.Sprintf("m.gpu_ram_gb >= $%d", argIdx))
		args = append(args, f.MinVRAMGB)
		argIdx++
	}
	if f.MaxPriceCents > 0 {
		where = append(where, fmt.Sprintf("m.price_per_hour_cents <= $%d", argIdx))
		args = append(args, f.MaxPriceCents)
		argIdx++
	}
	if f.MinPriceCents > 0 {
		where = append(where, fmt.Sprintf("m.price_per_hour_cents >= $%d", argIdx))
		args = append(args, f.MinPriceCents)
		argIdx++
	}
	if f.Country != "" {
		where = append(where, fmt.Sprintf("m.country = $%d", argIdx))
		args = append(args, f.Country)
		argIdx++
	}

	whereClause := strings.Join(where, " AND ")

	var total int
	countQuery := fmt.Sprintf("SELECT COUNT(*) FROM machines m WHERE %s", whereClause)
	if err := r.db.QueryRow(ctx, countQuery, args...).Scan(&total); err != nil {
		return nil, 0, err
	}

	orderBy := "m.price_per_hour_cents ASC"
	if f.SortBy == "gpu_ram" {
		orderBy = "m.gpu_ram_gb DESC"
	} else if f.SortBy == "reliability" {
		orderBy = "m.uptime_percentage DESC"
	}
	if f.SortDir == "desc" && f.SortBy == "price" {
		orderBy = "m.price_per_hour_cents DESC"
	}

	limit := f.Limit
	if limit <= 0 {
		limit = 50
	}

	query := fmt.Sprintf(
		`SELECT m.id, m.host_id, m.label, m.status, m.gpu_model_id, m.gpu_count, m.gpu_ram_gb,
				m.cpu_model, m.cpu_cores, m.ram_gb, m.disk_gb, m.disk_type,
				m.country, m.region, m.cuda_version, m.price_per_hour_cents,
				m.uptime_percentage, m.total_rentals, m.avg_rating
		 FROM machines m WHERE %s ORDER BY %s LIMIT $%d OFFSET $%d`,
		whereClause, orderBy, argIdx, argIdx+1)
	args = append(args, limit, f.Offset)

	rows, err := r.db.Query(ctx, query, args...)
	if err != nil {
		return nil, 0, err
	}
	defer rows.Close()

	var machines []model.Machine
	for rows.Next() {
		var m model.Machine
		if err := rows.Scan(
			&m.ID, &m.HostID, &m.Label, &m.Status, &m.GPUModelID, &m.GPUCount,
			&m.GPURAMGB, &m.CPUModel, &m.CPUCores, &m.RAMGB, &m.DiskGB, &m.DiskType,
			&m.Country, &m.Region, &m.CUDAVersion, &m.PricePerHourCents,
			&m.UptimePercentage, &m.TotalRentals, &m.AvgRating,
		); err != nil {
			return nil, 0, err
		}
		machines = append(machines, m)
	}
	return machines, total, nil
}

func (r *MachineRepo) UpdateHeartbeat(ctx context.Context, machineID uuid.UUID, hb model.MachineHeartbeat) error {
	_, err := r.db.Exec(ctx,
		`UPDATE machines SET status = 'online', last_heartbeat = now(), agent_version = $2 WHERE id = $1`,
		machineID, hb.AgentVersion)
	if err != nil {
		return err
	}
	_, err = r.db.Exec(ctx,
		`INSERT INTO machine_heartbeats (machine_id, gpu_utilization, gpu_temp_c, gpu_memory_used_mb,
			gpu_memory_total_mb, cpu_utilization, ram_used_mb, ram_total_mb, disk_used_gb, disk_total_gb,
			upload_mbps, download_mbps)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)`,
		machineID, hb.GPUUtilization, hb.GPUTempC, hb.GPUMemUsedMB, hb.GPUMemTotalMB,
		hb.CPUUtilization, hb.RAMUsedMB, hb.RAMTotalMB, hb.DiskUsedGB, hb.DiskTotalGB,
		hb.UploadMbps, hb.DownloadMbps)
	return err
}

func (r *MachineRepo) Delete(ctx context.Context, id uuid.UUID, hostID uuid.UUID) error {
	_, err := r.db.Exec(ctx, `DELETE FROM machines WHERE id = $1 AND host_id = $2`, id, hostID)
	return err
}

func (r *MachineRepo) MarkStale(ctx context.Context, staleSec int) (int64, error) {
	tag, err := r.db.Exec(ctx,
		`UPDATE machines SET status = 'offline'
		 WHERE status = 'online' AND last_heartbeat < now() - make_interval(secs => $1)`,
		staleSec)
	if err != nil {
		return 0, err
	}
	return tag.RowsAffected(), nil
}
