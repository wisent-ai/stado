package model

import (
	"time"

	"github.com/google/uuid"
)

type TransactionType string

const (
	TransactionPurchase     TransactionType = "purchase"
	TransactionRentalCharge TransactionType = "rental_charge"
	TransactionRentalRefund TransactionType = "rental_refund"
	TransactionPayout       TransactionType = "payout"
	TransactionBonus        TransactionType = "bonus"
	TransactionAdjustment   TransactionType = "adjustment"
)

type CreditTransaction struct {
	ID                    uuid.UUID       `json:"id" db:"id"`
	UserID                uuid.UUID       `json:"user_id" db:"user_id"`
	AmountCents           int64           `json:"amount_cents" db:"amount_cents"`
	Type                  TransactionType `json:"type" db:"type"`
	Description           string          `json:"description" db:"description"`
	InstanceID            *uuid.UUID      `json:"instance_id,omitempty" db:"instance_id"`
	StripePaymentIntentID *string         `json:"stripe_payment_intent_id,omitempty" db:"stripe_payment_intent_id"`
	BalanceAfterCents     int64           `json:"balance_after_cents" db:"balance_after_cents"`
	CreatedAt             time.Time       `json:"created_at" db:"created_at"`
}

type CreditPackage struct {
	ID            string `json:"id"`
	Name          string `json:"name"`
	CreditsCents  int64  `json:"credits_cents"`
	PriceCents    int64  `json:"price_cents"`
	BonusPercent  int    `json:"bonus_percent"`
}

var CreditPackages = []CreditPackage{
	{ID: "starter", Name: "Starter", CreditsCents: 1000, PriceCents: 1000, BonusPercent: 0},
	{ID: "builder", Name: "Builder", CreditsCents: 5000, PriceCents: 4500, BonusPercent: 10},
	{ID: "pro", Name: "Pro", CreditsCents: 20000, PriceCents: 17000, BonusPercent: 15},
	{ID: "enterprise", Name: "Enterprise", CreditsCents: 100000, PriceCents: 80000, BonusPercent: 20},
}

type CheckoutRequest struct {
	PackageID string `json:"package_id" validate:"required"`
}

type CheckoutResponse struct {
	CheckoutURL string `json:"checkout_url"`
}

type PayoutStatus string

const (
	PayoutPending    PayoutStatus = "pending"
	PayoutProcessing PayoutStatus = "processing"
	PayoutCompleted  PayoutStatus = "completed"
	PayoutFailed     PayoutStatus = "failed"
)

type HostEarning struct {
	ID              uuid.UUID `json:"id" db:"id"`
	HostID          uuid.UUID `json:"host_id" db:"host_id"`
	InstanceID      uuid.UUID `json:"instance_id" db:"instance_id"`
	AmountCents     int64     `json:"amount_cents" db:"amount_cents"`
	PlatformFeeCents int64    `json:"platform_fee_cents" db:"platform_fee_cents"`
	NetAmountCents  int64     `json:"net_amount_cents" db:"net_amount_cents"`
	PeriodStart     time.Time `json:"period_start" db:"period_start"`
	PeriodEnd       time.Time `json:"period_end" db:"period_end"`
	CreatedAt       time.Time `json:"created_at" db:"created_at"`
}

type Payout struct {
	ID               uuid.UUID    `json:"id" db:"id"`
	HostID           uuid.UUID    `json:"host_id" db:"host_id"`
	AmountCents      int64        `json:"amount_cents" db:"amount_cents"`
	Status           PayoutStatus `json:"status" db:"status"`
	StripeTransferID *string      `json:"stripe_transfer_id,omitempty" db:"stripe_transfer_id"`
	PayoutMethod     string       `json:"payout_method" db:"payout_method"`
	RequestedAt      time.Time    `json:"requested_at" db:"requested_at"`
	CompletedAt      *time.Time   `json:"completed_at,omitempty" db:"completed_at"`
}

type PayoutSettings struct {
	HostID                  uuid.UUID `json:"host_id" db:"host_id"`
	StripeConnectAccountID  *string   `json:"stripe_connect_account_id,omitempty" db:"stripe_connect_account_id"`
	PayoutThresholdCents    int64     `json:"payout_threshold_cents" db:"payout_threshold_cents"`
	AutoPayout              bool      `json:"auto_payout" db:"auto_payout"`
}

type EarningsSummary struct {
	TotalEarnedCents    int64 `json:"total_earned_cents"`
	PendingPayoutCents  int64 `json:"pending_payout_cents"`
	TotalPaidOutCents   int64 `json:"total_paid_out_cents"`
	ThisMonthCents      int64 `json:"this_month_cents"`
}
