package scheduler

import (
	"context"
	"time"

	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
)

type BillingTicker struct {
	instanceRepo *repository.InstanceRepo
	billingRepo  *repository.BillingRepo
}

func NewBillingTicker(ir *repository.InstanceRepo, br *repository.BillingRepo) *BillingTicker {
	return &BillingTicker{instanceRepo: ir, billingRepo: br}
}

func (bt *BillingTicker) Start(ctx context.Context) {
	ticker := time.NewTicker(60 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			bt.tick(ctx)
		}
	}
}

func (bt *BillingTicker) tick(ctx context.Context) {
	instances, err := bt.instanceRepo.ListRunning(ctx)
	if err != nil {
		log.Error().Err(err).Msg("billing ticker: failed to list running instances")
		return
	}

	now := time.Now()
	for _, inst := range instances {
		billedAt := inst.LastBilledAt
		if billedAt == nil {
			billedAt = inst.StartedAt
		}
		if billedAt == nil {
			continue
		}

		if now.Sub(*billedAt) < time.Hour {
			continue
		}

		amount := int64(inst.PricePerHourCents)
		desc := "Hourly charge for instance " + inst.ID.String()
		instID := inst.ID

		success, err := bt.billingRepo.DeductCredits(ctx, inst.UserID, amount, desc, &instID)
		if err != nil {
			log.Error().Err(err).Str("instance", inst.ID.String()).Msg("billing deduction failed")
			continue
		}

		if !success {
			log.Warn().Str("instance", inst.ID.String()).Msg("insufficient credits, stopping instance")
			_ = bt.instanceRepo.UpdateStatus(ctx, inst.ID, "stopping")
			_ = bt.instanceRepo.InsertEvent(ctx, inst.ID, "auto_stopped", map[string]string{"reason": "insufficient_credits"})
			continue
		}

		_ = bt.instanceRepo.RecordBilling(ctx, inst.ID, amount)

		platformFee := amount * 15 / 100
		netAmount := amount - platformFee
		_ = bt.billingRepo.RecordHostEarning(ctx, inst.HostID, inst.ID, amount, platformFee, netAmount, *billedAt, now)

		log.Info().Str("instance", inst.ID.String()).Int64("amount", amount).Msg("billed instance")
	}
}

// Ensure uuid import is used
var _ = uuid.Nil
