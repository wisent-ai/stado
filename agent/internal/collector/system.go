package collector

import (
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"syscall"
)

type SystemInfo struct {
	CPUModel    string  `json:"cpu_model"`
	CPUCores    int     `json:"cpu_cores"`
	CPUUtil     float32 `json:"cpu_utilization"`
	RAMTotalMB  int     `json:"ram_total_mb"`
	RAMUsedMB   int     `json:"ram_used_mb"`
	DiskTotalGB int     `json:"disk_total_gb"`
	DiskUsedGB  int     `json:"disk_used_gb"`
	OSVersion   string  `json:"os"`
	UploadMbps  float32 `json:"upload_mbps"`
	DownloadMbps float32 `json:"download_mbps"`
}

func CollectSystem() (*SystemInfo, error) {
	info := &SystemInfo{
		CPUCores: runtime.NumCPU(),
	}

	if data, err := os.ReadFile("/proc/cpuinfo"); err == nil {
		for _, line := range strings.Split(string(data), "\n") {
			if strings.HasPrefix(line, "model name") {
				parts := strings.SplitN(line, ":", 2)
				if len(parts) > 1 {
					info.CPUModel = strings.TrimSpace(parts[1])
				}
				break
			}
		}
	}

	if data, err := os.ReadFile("/proc/meminfo"); err == nil {
		for _, line := range strings.Split(string(data), "\n") {
			if strings.HasPrefix(line, "MemTotal:") {
				fields := strings.Fields(line)
				if len(fields) >= 2 {
					kb, _ := strconv.Atoi(fields[1])
					info.RAMTotalMB = kb / 1024
				}
			}
			if strings.HasPrefix(line, "MemAvailable:") {
				fields := strings.Fields(line)
				if len(fields) >= 2 {
					kb, _ := strconv.Atoi(fields[1])
					availMB := kb / 1024
					info.RAMUsedMB = info.RAMTotalMB - availMB
				}
			}
		}
	}

	var stat syscall.Statfs_t
	if err := syscall.Statfs("/", &stat); err == nil {
		info.DiskTotalGB = int(stat.Blocks * uint64(stat.Bsize) / (1024 * 1024 * 1024))
		freeGB := int(stat.Bfree * uint64(stat.Bsize) / (1024 * 1024 * 1024))
		info.DiskUsedGB = info.DiskTotalGB - freeGB
	}

	if out, err := exec.Command("cat", "/etc/os-release").Output(); err == nil {
		for _, line := range strings.Split(string(out), "\n") {
			if strings.HasPrefix(line, "PRETTY_NAME=") {
				info.OSVersion = strings.Trim(strings.TrimPrefix(line, "PRETTY_NAME="), `"`)
				break
			}
		}
	}

	return info, nil
}
