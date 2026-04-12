package model

import (
	"net/netip"
	"time"

	"github.com/google/uuid"
)

type MachineStatus string

const (
	MachineStatusPending     MachineStatus = "pending"
	MachineStatusOnline      MachineStatus = "online"
	MachineStatusOffline     MachineStatus = "offline"
	MachineStatusMaintenance MachineStatus = "maintenance"
	MachineStatusBanned      MachineStatus = "banned"
)

type GPUModel struct {
	ID                   int     `json:"id" db:"id"`
	Name                 string  `json:"name" db:"name"`
	Manufacturer         string  `json:"manufacturer" db:"manufacturer"`
	VRAMGB               int     `json:"vram_gb" db:"vram_gb"`
	Architecture         string  `json:"architecture,omitempty" db:"architecture"`
	FP16TFLOPS           float32 `json:"fp16_tflops,omitempty" db:"fp16_tflops"`
	FP32TFLOPS           float32 `json:"fp32_tflops,omitempty" db:"fp32_tflops"`
	MemoryBandwidthGBPS  float32 `json:"memory_bandwidth_gbps,omitempty" db:"memory_bandwidth_gbps"`
	TDPWatts             int     `json:"tdp_watts,omitempty" db:"tdp_watts"`
}

type Machine struct {
	ID                uuid.UUID     `json:"id" db:"id"`
	HostID            uuid.UUID     `json:"host_id" db:"host_id"`
	Hostname          string        `json:"hostname,omitempty" db:"hostname"`
	Label             string        `json:"label,omitempty" db:"label"`
	Status            MachineStatus `json:"status" db:"status"`
	GPUModelID        *int          `json:"gpu_model_id,omitempty" db:"gpu_model_id"`
	GPUModel          *GPUModel     `json:"gpu_model,omitempty" db:"-"`
	GPUCount          int           `json:"gpu_count" db:"gpu_count"`
	GPURAMGB          int           `json:"gpu_ram_gb,omitempty" db:"gpu_ram_gb"`
	CPUModel          string        `json:"cpu_model,omitempty" db:"cpu_model"`
	CPUCores          int           `json:"cpu_cores,omitempty" db:"cpu_cores"`
	RAMGB             int           `json:"ram_gb,omitempty" db:"ram_gb"`
	DiskGB            int           `json:"disk_gb,omitempty" db:"disk_gb"`
	DiskType          string        `json:"disk_type,omitempty" db:"disk_type"`
	UploadMbps        float32       `json:"upload_mbps,omitempty" db:"upload_mbps"`
	DownloadMbps      float32       `json:"download_mbps,omitempty" db:"download_mbps"`
	Country           string        `json:"country,omitempty" db:"country"`
	Region            string        `json:"region,omitempty" db:"region"`
	IPAddress         *netip.Addr   `json:"ip_address,omitempty" db:"ip_address"`
	CUDAVersion       string        `json:"cuda_version,omitempty" db:"cuda_version"`
	DockerVersion     string        `json:"docker_version,omitempty" db:"docker_version"`
	OSVersion         string        `json:"os_version,omitempty" db:"os_version"`
	DriverVersion     string        `json:"driver_version,omitempty" db:"driver_version"`
	PricePerHourCents int           `json:"price_per_hour_cents" db:"price_per_hour_cents"`
	MinRentalHours    int           `json:"min_rental_hours" db:"min_rental_hours"`
	MaxRentalHours    *int          `json:"max_rental_hours,omitempty" db:"max_rental_hours"`
	AgentVersion      string        `json:"agent_version,omitempty" db:"agent_version"`
	LastHeartbeat     *time.Time    `json:"last_heartbeat,omitempty" db:"last_heartbeat"`
	UptimePercentage  float32       `json:"uptime_percentage" db:"uptime_percentage"`
	TotalRentals      int           `json:"total_rentals" db:"total_rentals"`
	AvgRating         *float32      `json:"avg_rating,omitempty" db:"avg_rating"`
	IsAvailable       bool          `json:"is_available" db:"is_available"`
	CurrentInstanceID *uuid.UUID    `json:"current_instance_id,omitempty" db:"current_instance_id"`
	CreatedAt         time.Time     `json:"created_at" db:"created_at"`
	UpdatedAt         time.Time     `json:"updated_at" db:"updated_at"`
}

type RegisterMachineParams struct {
	Label            string `json:"label" validate:"required"`
	PricePerHourCents int   `json:"price_per_hour_cents" validate:"required,min=1"`
	MinRentalHours   int    `json:"min_rental_hours"`
	MaxRentalHours   *int   `json:"max_rental_hours,omitempty"`
	Country          string `json:"country"`
	Region           string `json:"region"`
}

type RegisterMachineResponse struct {
	Machine
	AgentToken string `json:"agent_token"`
}

type UpdateMachineParams struct {
	Label             *string `json:"label,omitempty"`
	PricePerHourCents *int    `json:"price_per_hour_cents,omitempty"`
	MinRentalHours    *int    `json:"min_rental_hours,omitempty"`
	MaxRentalHours    *int    `json:"max_rental_hours,omitempty"`
	IsAvailable       *bool   `json:"is_available,omitempty"`
}

type MachineHeartbeat struct {
	MachineID       uuid.UUID `json:"machine_id"`
	AgentVersion    string    `json:"agent_version"`
	GPUUtilization  float32   `json:"gpu_utilization"`
	GPUTempC        float32   `json:"gpu_temp_c"`
	GPUMemUsedMB    int       `json:"gpu_memory_used_mb"`
	GPUMemTotalMB   int       `json:"gpu_memory_total_mb"`
	CPUUtilization  float32   `json:"cpu_utilization"`
	RAMUsedMB       int       `json:"ram_used_mb"`
	RAMTotalMB      int       `json:"ram_total_mb"`
	DiskUsedGB      int       `json:"disk_used_gb"`
	DiskTotalGB     int       `json:"disk_total_gb"`
	UploadMbps      float32   `json:"upload_mbps"`
	DownloadMbps    float32   `json:"download_mbps"`
}
