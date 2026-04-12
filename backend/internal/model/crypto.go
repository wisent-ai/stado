package model

import (
	"time"

	"github.com/google/uuid"
)

type CryptoTxStatus string

const (
	CryptoTxPending   CryptoTxStatus = "pending"
	CryptoTxConfirmed CryptoTxStatus = "confirmed"
	CryptoTxCredited  CryptoTxStatus = "credited"
	CryptoTxFailed    CryptoTxStatus = "failed"
)

type CryptoDepositAddress struct {
	ID        uuid.UUID `json:"id" db:"id"`
	UserID    uuid.UUID `json:"user_id" db:"user_id"`
	Chain     string    `json:"chain" db:"chain"`
	Address   string    `json:"address" db:"address"`
	HDIndex   int       `json:"hd_index" db:"hd_index"`
	CreatedAt time.Time `json:"created_at" db:"created_at"`
}

type CryptoDeposit struct {
	ID             uuid.UUID      `json:"id" db:"id"`
	UserID         uuid.UUID      `json:"user_id" db:"user_id"`
	Chain          string         `json:"chain" db:"chain"`
	TxHash         string         `json:"tx_hash" db:"tx_hash"`
	TokenAddress   string         `json:"token_address" db:"token_address"`
	TokenSymbol    string         `json:"token_symbol" db:"token_symbol"`
	AmountWei      string         `json:"amount_wei" db:"amount_wei"`
	AmountUSDCents int64          `json:"amount_usd_cents" db:"amount_usd_cents"`
	Status         CryptoTxStatus `json:"status" db:"status"`
	BlockNumber    int64          `json:"block_number" db:"block_number"`
	Confirmations  int            `json:"confirmations" db:"confirmations"`
	CreditedAt     *time.Time     `json:"credited_at,omitempty" db:"credited_at"`
	CreatedAt      time.Time      `json:"created_at" db:"created_at"`
}

type CryptoWithdrawal struct {
	ID             uuid.UUID      `json:"id" db:"id"`
	UserID         uuid.UUID      `json:"user_id" db:"user_id"`
	Chain          string         `json:"chain" db:"chain"`
	ToAddress      string         `json:"to_address" db:"to_address"`
	TokenAddress   string         `json:"token_address" db:"token_address"`
	TokenSymbol    string         `json:"token_symbol" db:"token_symbol"`
	AmountWei      string         `json:"amount_wei" db:"amount_wei"`
	AmountUSDCents int64          `json:"amount_usd_cents" db:"amount_usd_cents"`
	TxHash         *string        `json:"tx_hash,omitempty" db:"tx_hash"`
	Status         CryptoTxStatus `json:"status" db:"status"`
	CreatedAt      time.Time      `json:"created_at" db:"created_at"`
	CompletedAt    *time.Time     `json:"completed_at,omitempty" db:"completed_at"`
}

type WithdrawRequest struct {
	ToAddress    string `json:"to_address" validate:"required"`
	AmountCents  int64  `json:"amount_cents" validate:"required,min=100"`
}

type CryptoPrice struct {
	Token        string `json:"token"`
	Symbol       string `json:"symbol"`
	Chain        string `json:"chain"`
	PriceUSDCents int64 `json:"price_usd_cents"`
}
