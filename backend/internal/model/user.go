package model

import (
	"time"

	"github.com/google/uuid"
)

type UserRole string

const (
	RoleUser  UserRole = "user"
	RoleHost  UserRole = "host"
	RoleAdmin UserRole = "admin"
)

type Profile struct {
	ID                 uuid.UUID `json:"id" db:"id"`
	Email              string    `json:"email" db:"email"`
	FullName           string    `json:"full_name" db:"full_name"`
	AvatarURL          string    `json:"avatar_url" db:"avatar_url"`
	Role               UserRole  `json:"role" db:"role"`
	StripeCustomerID   *string   `json:"stripe_customer_id,omitempty" db:"stripe_customer_id"`
	CreditBalanceCents int64     `json:"credit_balance_cents" db:"credit_balance_cents"`
	IsHost             bool      `json:"is_host" db:"is_host"`
	CreatedAt          time.Time `json:"created_at" db:"created_at"`
	UpdatedAt          time.Time `json:"updated_at" db:"updated_at"`
}

type UpdateProfileParams struct {
	FullName  *string `json:"full_name,omitempty"`
	AvatarURL *string `json:"avatar_url,omitempty"`
}

type APIKey struct {
	ID         uuid.UUID  `json:"id" db:"id"`
	UserID     uuid.UUID  `json:"user_id" db:"user_id"`
	Name       string     `json:"name" db:"name"`
	KeyPrefix  string     `json:"key_prefix" db:"key_prefix"`
	KeyHash    string     `json:"-" db:"key_hash"`
	LastUsedAt *time.Time `json:"last_used_at,omitempty" db:"last_used_at"`
	ExpiresAt  *time.Time `json:"expires_at,omitempty" db:"expires_at"`
	CreatedAt  time.Time  `json:"created_at" db:"created_at"`
	RevokedAt  *time.Time `json:"revoked_at,omitempty" db:"revoked_at"`
}

type CreateAPIKeyParams struct {
	Name string `json:"name" validate:"required"`
}

type CreateAPIKeyResponse struct {
	APIKey
	PlaintextKey string `json:"key"`
}
