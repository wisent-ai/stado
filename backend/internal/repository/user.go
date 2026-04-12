package repository

import (
	"context"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
)

type UserRepo struct {
	db *pgxpool.Pool
}

func NewUserRepo(db *pgxpool.Pool) *UserRepo {
	return &UserRepo{db: db}
}

func (r *UserRepo) GetByID(ctx context.Context, id uuid.UUID) (*model.Profile, error) {
	p := &model.Profile{}
	err := r.db.QueryRow(ctx,
		`SELECT id, email, full_name, avatar_url, role, stripe_customer_id,
				credit_balance_cents, is_host, created_at, updated_at
		 FROM profiles WHERE id = $1`, id).Scan(
		&p.ID, &p.Email, &p.FullName, &p.AvatarURL, &p.Role, &p.StripeCustomerID,
		&p.CreditBalanceCents, &p.IsHost, &p.CreatedAt, &p.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}
	return p, nil
}

func (r *UserRepo) Create(ctx context.Context, id uuid.UUID, email string) (*model.Profile, error) {
	p := &model.Profile{}
	err := r.db.QueryRow(ctx,
		`INSERT INTO profiles (id, email) VALUES ($1, $2)
		 ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email
		 RETURNING id, email, full_name, avatar_url, role, stripe_customer_id,
				  credit_balance_cents, is_host, created_at, updated_at`,
		id, email).Scan(
		&p.ID, &p.Email, &p.FullName, &p.AvatarURL, &p.Role, &p.StripeCustomerID,
		&p.CreditBalanceCents, &p.IsHost, &p.CreatedAt, &p.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}
	return p, nil
}

func (r *UserRepo) Update(ctx context.Context, id uuid.UUID, params model.UpdateProfileParams) (*model.Profile, error) {
	p := &model.Profile{}
	err := r.db.QueryRow(ctx,
		`UPDATE profiles SET
			full_name = COALESCE($2, full_name),
			avatar_url = COALESCE($3, avatar_url)
		 WHERE id = $1
		 RETURNING id, email, full_name, avatar_url, role, stripe_customer_id,
				  credit_balance_cents, is_host, created_at, updated_at`,
		id, params.FullName, params.AvatarURL).Scan(
		&p.ID, &p.Email, &p.FullName, &p.AvatarURL, &p.Role, &p.StripeCustomerID,
		&p.CreditBalanceCents, &p.IsHost, &p.CreatedAt, &p.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}
	return p, nil
}

func (r *UserRepo) SetHost(ctx context.Context, id uuid.UUID) error {
	_, err := r.db.Exec(ctx,
		`UPDATE profiles SET is_host = true, role = 'host' WHERE id = $1`, id)
	return err
}
