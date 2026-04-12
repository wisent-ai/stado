package scheduler

import (
	"context"
	"time"

	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
)

type StaleChecker struct {
	machineRepo *repository.MachineRepo
	staleSec    int
}

func NewStaleChecker(mr *repository.MachineRepo, staleSec int) *StaleChecker {
	return &StaleChecker{machineRepo: mr, staleSec: staleSec}
}

func (sc *StaleChecker) Start(ctx context.Context) {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			count, err := sc.machineRepo.MarkStale(ctx, sc.staleSec)
			if err != nil {
				log.Error().Err(err).Msg("stale checker failed")
			} else if count > 0 {
				log.Info().Int64("count", count).Msg("marked machines as offline")
			}
		}
	}
}
