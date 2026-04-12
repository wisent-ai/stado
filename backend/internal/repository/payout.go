package repository

import (
	"context"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
)

type PayoutRepo struct {
	db *pgxpool.Pool
}

func NewPayoutRepo(db *pgxpool.Pool) *PayoutRepo {
	return &PayoutRepo{db: db}
}

func (r *PayoutRepo) Create(ctx context.Context, hostID uuid.UUID, amountCents int64, method string) (*model.Payout, error) {
	p := &model.Payout{}
	err := r.db.QueryRow(ctx,
		`INSERT INTO payouts (host_id, amount_cents, payout_method)
		 VALUES ($1, $2, $3)
		 RETURNING id, host_id, amount_cents, status, payout_method, requested_at`,
		hostID, amountCents, method).Scan(
		&p.ID, &p.HostID, &p.AmountCents, &p.Status, &p.PayoutMethod, &p.RequestedAt,
	)
	if err != nil {
		return nil, err
	}
	return p, nil
}

func (r *PayoutRepo) ListByHost(ctx context.Context, hostID uuid.UUID) ([]model.Payout, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, host_id, amount_cents, status, stripe_transfer_id, payout_method,
				requested_at, completed_at
		 FROM payouts WHERE host_id = $1 ORDER BY requested_at DESC`, hostID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var payouts []model.Payout
	for rows.Next() {
		var p model.Payout
		if err := rows.Scan(
			&p.ID, &p.HostID, &p.AmountCents, &p.Status, &p.StripeTransferID,
			&p.PayoutMethod, &p.RequestedAt, &p.CompletedAt,
		); err != nil {
			return nil, err
		}
		payouts = append(payouts, p)
	}
	return payouts, nil
}

func (r *PayoutRepo) UpdateStatus(ctx context.Context, id uuid.UUID, status model.PayoutStatus, transferID *string) error {
	_, err := r.db.Exec(ctx,
		`UPDATE payouts SET status = $2, stripe_transfer_id = $3,
			completed_at = CASE WHEN $2 = 'completed' THEN now() ELSE completed_at END
		 WHERE id = $1`,
		id, status, transferID)
	return err
}

func (r *PayoutRepo) GetSettings(ctx context.Context, hostID uuid.UUID) (*model.PayoutSettings, error) {
	s := &model.PayoutSettings{}
	err := r.db.QueryRow(ctx,
		`SELECT host_id, stripe_connect_account_id, payout_threshold_cents, auto_payout
		 FROM host_payout_settings WHERE host_id = $1`, hostID).Scan(
		&s.HostID, &s.StripeConnectAccountID, &s.PayoutThresholdCents, &s.AutoPayout,
	)
	if err != nil {
		return nil, err
	}
	return s, nil
}

func (r *PayoutRepo) UpsertSettings(ctx context.Context, s model.PayoutSettings) error {
	_, err := r.db.Exec(ctx,
		`INSERT INTO host_payout_settings (host_id, stripe_connect_account_id, payout_threshold_cents, auto_payout)
		 VALUES ($1, $2, $3, $4)
		 ON CONFLICT (host_id) DO UPDATE SET
			stripe_connect_account_id = EXCLUDED.stripe_connect_account_id,
			payout_threshold_cents = EXCLUDED.payout_threshold_cents,
			auto_payout = EXCLUDED.auto_payout,
			updated_at = now()`,
		s.HostID, s.StripeConnectAccountID, s.PayoutThresholdCents, s.AutoPayout)
	return err
}
