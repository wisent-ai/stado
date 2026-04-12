package repository

import (
	"context"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
)

type CryptoRepo struct {
	db *pgxpool.Pool
}

func NewCryptoRepo(db *pgxpool.Pool) *CryptoRepo {
	return &CryptoRepo{db: db}
}

func (r *CryptoRepo) GetDepositAddress(ctx context.Context, userID uuid.UUID, chain string) (*model.CryptoDepositAddress, error) {
	a := &model.CryptoDepositAddress{}
	err := r.db.QueryRow(ctx,
		`SELECT id, user_id, chain, address, hd_index, created_at
		 FROM crypto_deposit_addresses WHERE user_id = $1 AND chain = $2`, userID, chain).Scan(
		&a.ID, &a.UserID, &a.Chain, &a.Address, &a.HDIndex, &a.CreatedAt)
	if err != nil {
		return nil, err
	}
	return a, nil
}

func (r *CryptoRepo) CreateDepositAddress(ctx context.Context, userID uuid.UUID, chain, address string, hdIndex int) (*model.CryptoDepositAddress, error) {
	a := &model.CryptoDepositAddress{}
	err := r.db.QueryRow(ctx,
		`INSERT INTO crypto_deposit_addresses (user_id, chain, address, hd_index)
		 VALUES ($1, $2, $3, $4)
		 RETURNING id, user_id, chain, address, hd_index, created_at`,
		userID, chain, address, hdIndex).Scan(
		&a.ID, &a.UserID, &a.Chain, &a.Address, &a.HDIndex, &a.CreatedAt)
	if err != nil {
		return nil, err
	}
	return a, nil
}

func (r *CryptoRepo) GetNextHDIndex(ctx context.Context, chain string) (int, error) {
	var idx int
	err := r.db.QueryRow(ctx,
		`SELECT COALESCE(MAX(hd_index), -1) + 1 FROM crypto_deposit_addresses WHERE chain = $1`, chain).Scan(&idx)
	return idx, err
}

func (r *CryptoRepo) GetAllDepositAddresses(ctx context.Context, chain string) ([]model.CryptoDepositAddress, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, user_id, chain, address, hd_index, created_at
		 FROM crypto_deposit_addresses WHERE chain = $1`, chain)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var addrs []model.CryptoDepositAddress
	for rows.Next() {
		var a model.CryptoDepositAddress
		if err := rows.Scan(&a.ID, &a.UserID, &a.Chain, &a.Address, &a.HDIndex, &a.CreatedAt); err != nil {
			return nil, err
		}
		addrs = append(addrs, a)
	}
	return addrs, nil
}

func (r *CryptoRepo) InsertDeposit(ctx context.Context, d model.CryptoDeposit) (*model.CryptoDeposit, error) {
	result := &model.CryptoDeposit{}
	err := r.db.QueryRow(ctx,
		`INSERT INTO crypto_deposits (user_id, chain, tx_hash, token_address, token_symbol, amount_wei, amount_usd_cents, status, block_number, confirmations)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
		 ON CONFLICT (tx_hash) DO UPDATE SET confirmations = EXCLUDED.confirmations, status = EXCLUDED.status
		 RETURNING id, user_id, chain, tx_hash, token_symbol, amount_wei, amount_usd_cents, status, block_number, confirmations, created_at`,
		d.UserID, d.Chain, d.TxHash, d.TokenAddress, d.TokenSymbol, d.AmountWei, d.AmountUSDCents, d.Status, d.BlockNumber, d.Confirmations).Scan(
		&result.ID, &result.UserID, &result.Chain, &result.TxHash, &result.TokenSymbol,
		&result.AmountWei, &result.AmountUSDCents, &result.Status, &result.BlockNumber, &result.Confirmations, &result.CreatedAt)
	if err != nil {
		return nil, err
	}
	return result, nil
}

func (r *CryptoRepo) MarkDepositCredited(ctx context.Context, depositID uuid.UUID) error {
	_, err := r.db.Exec(ctx,
		`UPDATE crypto_deposits SET status = 'credited', credited_at = now() WHERE id = $1`, depositID)
	return err
}

func (r *CryptoRepo) ListPendingDeposits(ctx context.Context, chain string) ([]model.CryptoDeposit, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, user_id, chain, tx_hash, token_address, token_symbol, amount_wei, amount_usd_cents, status, block_number, confirmations, created_at
		 FROM crypto_deposits WHERE chain = $1 AND status IN ('pending', 'confirmed') ORDER BY block_number`, chain)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var deposits []model.CryptoDeposit
	for rows.Next() {
		var d model.CryptoDeposit
		if err := rows.Scan(&d.ID, &d.UserID, &d.Chain, &d.TxHash, &d.TokenAddress, &d.TokenSymbol,
			&d.AmountWei, &d.AmountUSDCents, &d.Status, &d.BlockNumber, &d.Confirmations, &d.CreatedAt); err != nil {
			return nil, err
		}
		deposits = append(deposits, d)
	}
	return deposits, nil
}

func (r *CryptoRepo) ListUserDeposits(ctx context.Context, userID uuid.UUID) ([]model.CryptoDeposit, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, user_id, chain, tx_hash, token_symbol, amount_wei, amount_usd_cents, status, block_number, confirmations, credited_at, created_at
		 FROM crypto_deposits WHERE user_id = $1 ORDER BY created_at DESC`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var deposits []model.CryptoDeposit
	for rows.Next() {
		var d model.CryptoDeposit
		if err := rows.Scan(&d.ID, &d.UserID, &d.Chain, &d.TxHash, &d.TokenSymbol,
			&d.AmountWei, &d.AmountUSDCents, &d.Status, &d.BlockNumber, &d.Confirmations, &d.CreditedAt, &d.CreatedAt); err != nil {
			return nil, err
		}
		deposits = append(deposits, d)
	}
	return deposits, nil
}

func (r *CryptoRepo) InsertWithdrawal(ctx context.Context, w model.CryptoWithdrawal) (*model.CryptoWithdrawal, error) {
	result := &model.CryptoWithdrawal{}
	err := r.db.QueryRow(ctx,
		`INSERT INTO crypto_withdrawals (user_id, chain, to_address, token_address, token_symbol, amount_wei, amount_usd_cents)
		 VALUES ($1, $2, $3, $4, $5, $6, $7)
		 RETURNING id, user_id, chain, to_address, token_symbol, amount_wei, amount_usd_cents, status, created_at`,
		w.UserID, w.Chain, w.ToAddress, w.TokenAddress, w.TokenSymbol, w.AmountWei, w.AmountUSDCents).Scan(
		&result.ID, &result.UserID, &result.Chain, &result.ToAddress, &result.TokenSymbol,
		&result.AmountWei, &result.AmountUSDCents, &result.Status, &result.CreatedAt)
	if err != nil {
		return nil, err
	}
	return result, nil
}

func (r *CryptoRepo) CompleteWithdrawal(ctx context.Context, id uuid.UUID, txHash string) error {
	_, err := r.db.Exec(ctx,
		`UPDATE crypto_withdrawals SET status = 'confirmed', tx_hash = $2, completed_at = now() WHERE id = $1`,
		id, txHash)
	return err
}

func (r *CryptoRepo) ListUserWithdrawals(ctx context.Context, userID uuid.UUID) ([]model.CryptoWithdrawal, error) {
	rows, err := r.db.Query(ctx,
		`SELECT id, user_id, chain, to_address, token_symbol, amount_wei, amount_usd_cents, tx_hash, status, created_at, completed_at
		 FROM crypto_withdrawals WHERE user_id = $1 ORDER BY created_at DESC`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var wds []model.CryptoWithdrawal
	for rows.Next() {
		var w model.CryptoWithdrawal
		if err := rows.Scan(&w.ID, &w.UserID, &w.Chain, &w.ToAddress, &w.TokenSymbol,
			&w.AmountWei, &w.AmountUSDCents, &w.TxHash, &w.Status, &w.CreatedAt, &w.CompletedAt); err != nil {
			return nil, err
		}
		wds = append(wds, w)
	}
	return wds, nil
}
