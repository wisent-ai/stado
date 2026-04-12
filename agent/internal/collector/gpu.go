package collector

import (
	"os/exec"
	"strconv"
	"strings"
)

type GPUInfo struct {
	Index          int     `json:"index"`
	Name           string  `json:"name"`
	UUID           string  `json:"uuid"`
	VRAMTotalMB    int     `json:"vram_total_mb"`
	VRAMUsedMB     int     `json:"vram_used_mb"`
	Utilization    float32 `json:"utilization"`
	TemperatureC   float32 `json:"temperature_c"`
	PowerWatts     float32 `json:"power_watts"`
	DriverVersion  string  `json:"driver_version"`
	CUDAVersion    string  `json:"cuda_version"`
}

func CollectGPUs() ([]GPUInfo, error) {
	out, err := exec.Command("nvidia-smi",
		"--query-gpu=index,name,uuid,memory.total,memory.used,utilization.gpu,temperature.gpu,power.draw,driver_version",
		"--format=csv,noheader,nounits").Output()
	if err != nil {
		return nil, err
	}

	var gpus []GPUInfo
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		fields := strings.Split(line, ", ")
		if len(fields) < 9 {
			continue
		}

		idx, _ := strconv.Atoi(strings.TrimSpace(fields[0]))
		vramTotal, _ := strconv.Atoi(strings.TrimSpace(fields[3]))
		vramUsed, _ := strconv.Atoi(strings.TrimSpace(fields[4]))
		util, _ := strconv.ParseFloat(strings.TrimSpace(fields[5]), 32)
		temp, _ := strconv.ParseFloat(strings.TrimSpace(fields[6]), 32)
		power, _ := strconv.ParseFloat(strings.TrimSpace(fields[7]), 32)

		gpu := GPUInfo{
			Index:         idx,
			Name:          strings.TrimSpace(fields[1]),
			UUID:          strings.TrimSpace(fields[2]),
			VRAMTotalMB:   vramTotal,
			VRAMUsedMB:    vramUsed,
			Utilization:   float32(util),
			TemperatureC:  float32(temp),
			PowerWatts:    float32(power),
			DriverVersion: strings.TrimSpace(fields[8]),
		}
		gpus = append(gpus, gpu)
	}

	if len(gpus) > 0 {
		cudaOut, err := exec.Command("nvidia-smi", "--query-gpu=driver_version", "--format=csv,noheader").Output()
		if err == nil {
			nvccOut, err := exec.Command("nvcc", "--version").Output()
			if err == nil {
				for _, line := range strings.Split(string(nvccOut), "\n") {
					if strings.Contains(line, "release") {
						parts := strings.Split(line, "release ")
						if len(parts) > 1 {
							ver := strings.Split(parts[1], ",")[0]
							for i := range gpus {
								gpus[i].CUDAVersion = ver
							}
						}
					}
				}
			}
		}
		_ = cudaOut
	}

	return gpus, nil
}
