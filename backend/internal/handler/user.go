package handler

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/middleware"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
	"golang.org/x/crypto/bcrypt"
)

type UserHandler struct {
	userRepo *repository.UserRepo
	db       *pgxpool.Pool
}

func NewUserHandler(userRepo *repository.UserRepo, db *pgxpool.Pool) *UserHandler {
	return &UserHandler{userRepo: userRepo, db: db}
}

func (h *UserHandler) GetMe(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	profile, err := h.userRepo.GetByID(r.Context(), userID)
	if err != nil {
		http.Error(w, "profile not found", http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(profile)
}

func (h *UserHandler) UpdateMe(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	var params model.UpdateProfileParams
	if err := json.NewDecoder(r.Body).Decode(&params); err != nil {
		http.Error(w, "invalid request body", http.StatusBadRequest)
		return
	}
	profile, err := h.userRepo.Update(r.Context(), userID, params)
	if err != nil {
		http.Error(w, "update failed", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(profile)
}

func (h *UserHandler) AuthCallback(w http.ResponseWriter, r *http.Request) {
	var body struct {
		UserID string `json:"user_id"`
		Email  string `json:"email"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}
	id, err := uuid.Parse(body.UserID)
	if err != nil {
		http.Error(w, "invalid user_id", http.StatusBadRequest)
		return
	}
	profile, err := h.userRepo.Create(r.Context(), id, body.Email)
	if err != nil {
		log.Error().Err(err).Msg("failed to create profile")
		http.Error(w, "failed to create profile", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(profile)
}

func (h *UserHandler) ListAPIKeys(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	rows, err := h.db.Query(r.Context(),
		`SELECT id, user_id, name, key_prefix, last_used_at, expires_at, created_at
		 FROM api_keys WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC`, userID)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}
	defer rows.Close()

	var keys []model.APIKey
	for rows.Next() {
		var k model.APIKey
		if err := rows.Scan(&k.ID, &k.UserID, &k.Name, &k.KeyPrefix, &k.LastUsedAt, &k.ExpiresAt, &k.CreatedAt); err != nil {
			continue
		}
		keys = append(keys, k)
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(keys)
}

func (h *UserHandler) CreateAPIKey(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	var params model.CreateAPIKeyParams
	if err := json.NewDecoder(r.Body).Decode(&params); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}

	rawKey := make([]byte, 32)
	if _, err := rand.Read(rawKey); err != nil {
		http.Error(w, "key generation failed", http.StatusInternalServerError)
		return
	}
	plaintext := "wsk_" + hex.EncodeToString(rawKey)
	prefix := plaintext[:12]
	hash, err := bcrypt.GenerateFromPassword([]byte(plaintext), bcrypt.DefaultCost)
	if err != nil {
		http.Error(w, "hashing failed", http.StatusInternalServerError)
		return
	}

	var key model.APIKey
	err = h.db.QueryRow(r.Context(),
		`INSERT INTO api_keys (user_id, name, key_prefix, key_hash)
		 VALUES ($1, $2, $3, $4)
		 RETURNING id, user_id, name, key_prefix, created_at`,
		userID, params.Name, prefix, string(hash)).Scan(
		&key.ID, &key.UserID, &key.Name, &key.KeyPrefix, &key.CreatedAt)
	if err != nil {
		http.Error(w, "create failed", http.StatusInternalServerError)
		return
	}

	resp := model.CreateAPIKeyResponse{APIKey: key, PlaintextKey: plaintext}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(resp)
}

func (h *UserHandler) RevokeAPIKey(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	keyID, err := uuid.Parse(chi.URLParam(r, "keyID"))
	if err != nil {
		http.Error(w, "invalid key id", http.StatusBadRequest)
		return
	}
	_, err = h.db.Exec(r.Context(),
		`UPDATE api_keys SET revoked_at = now() WHERE id = $1 AND user_id = $2`, keyID, userID)
	if err != nil {
		http.Error(w, "revoke failed", http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
