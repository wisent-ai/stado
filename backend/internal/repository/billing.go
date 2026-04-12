package repository

import (
	"context"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
)

type BillingRepo struct {
	db *pgxpool.Pool
}

func NewBillingRepo(db *pgxpool.Pool) *BillingRepo {
	return &BillingRepo{db: db}
}

func (r *BillingRepo) GetBalance(ctx context.Context, userID uuid.UUID) (int64, error) {
	var balance int64
	err := r.db.QueryRow(ctx,
		`SELECT credit_balance_cents FROM profiles WHERE id = $1`, userID).Scan(&balance)
	return balance, err
}

func (r *BillingRepo) DeductCredits(ctx context.Context, userID uuid.UUID, amountCents int64, description string, instanceID *uuid.UUID) (bool, error) {
	var success bool
	err := r.db.QueryRow(ctx,
		`SELECT deduct_credits($1, $2, $3, $4)`,
		userID, amountCents, description, instanceID).Scan(&success)
	return success, err
}

func (r *BillingRepo) AddCredits(ctx context.Context, userID uuid.UUID, amountCents int64, txType model.TransactionType, description string, stripePI *string) (int64, error) {
	var newBalance int64
	err := r.db.QueryRow(ctx,
		`SELECT add_credits($1, $2, $3, $4, $5)`,
		userID, amountCents, txType, description, stripePI).Scan(&newBalance)
	return newBalance, err
}

func (r *BillingRepo) ListTransactions(ctx context.Context, userID uuid.UUID, limit, offset int) ([]model.CreditTransaction, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, user_id, amount_cents, type, description, instance_id,
				stripe_payment_intent_id, balance_after_cents, created_at
		 FROM credit_transactions WHERE user_id = $1
		 ORDER BY created_at DESC LIMIT $2 OFFSET $3`,
		userID, limit, offset)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var txns []model.CreditTransaction
	for rows.Next() {
		var t model.CreditTransaction
		if err := rows.Scan(
			&t.ID, &t.UserID, &t.AmountCents, &t.Type, &t.Description,
			&t.InstanceID, &t.StripePaymentIntentID, &t.BalanceAfterCents, &t.CreatedAt,
		); err != nil {
			return nil, err
		}
		txns = append(txns, t)
	}
	return txns, nil
}

func (r *BillingRepo) RecordHostEarning(ctx context.Context, hostID, instanceID uuid.UUID, grossCents, feeCents, netCents int64, periodStart, periodEnd interface{}) error {
	_, err := r.db.Exec(ctx,
		`INSERT INTO host_earnings (host_id, instance_id, amount_cents, platform_fee_cents, net_amount_cents, period_start, period_end)
		 VALUES ($1, $2, $3, $4, $5, $6, $7)`,
		hostID, instanceID, grossCents, feeCents, netCents, periodStart, periodEnd)
	return err
}

func (r *BillingRepo) GetEarningsSummary(ctx context.Context, hostID uuid.UUID) (*model.EarningsSummary, error) {
	s := &model.EarningsSummary{}
	err := r.db.QueryRow(ctx,
		`SELECT COALESCE(SUM(net_amount_cents), 0) FROM host_earnings WHERE host_id = $1`, hostID).Scan(&s.TotalEarnedCents)
	if err != nil {
		return nil, err
	}
	err = r.db.QueryRow(ctx,
		`SELECT COALESCE(SUM(amount_cents), 0) FROM payouts WHERE host_id = $1 AND status = 'completed'`, hostID).Scan(&s.TotalPaidOutCents)
	if err != nil {
		return nil, err
	}
	s.PendingPayoutCents = s.TotalEarnedCents - s.TotalPaidOutCents

	err = r.db.QueryRow(ctx,
		`SELECT COALESCE(SUM(net_amount_cents), 0) FROM host_earnings
		 WHERE host_id = $1 AND created_at >= date_trunc('month', now())`, hostID).Scan(&s.ThisMonthCents)
	return s, err
}
