package handler

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/middleware"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
	"golang.org/x/crypto/bcrypt"
)

type HostHandler struct {
	machineRepo *repository.MachineRepo
	billingRepo *repository.BillingRepo
	payoutRepo  *repository.PayoutRepo
	userRepo    *repository.UserRepo
	instanceRepo *repository.InstanceRepo
}

func NewHostHandler(mr *repository.MachineRepo, br *repository.BillingRepo, pr *repository.PayoutRepo, ur *repository.UserRepo, ir *repository.InstanceRepo) *HostHandler {
	return &HostHandler{machineRepo: mr, billingRepo: br, payoutRepo: pr, userRepo: ur, instanceRepo: ir}
}

func (h *HostHandler) ListMachines(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	machines, err := h.machineRepo.ListByHost(r.Context(), hostID)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(machines)
}

func (h *HostHandler) RegisterMachine(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())

	_ = h.userRepo.SetHost(r.Context(), hostID)

	var params model.RegisterMachineParams
	if err := json.NewDecoder(r.Body).Decode(&params); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}

	tokenBytes := make([]byte, 32)
	if _, err := rand.Read(tokenBytes); err != nil {
		http.Error(w, "token generation failed", http.StatusInternalServerError)
		return
	}
	plainToken := "wat_" + hex.EncodeToString(tokenBytes)
	hash, err := bcrypt.GenerateFromPassword([]byte(plainToken), bcrypt.DefaultCost)
	if err != nil {
		http.Error(w, "hashing failed", http.StatusInternalServerError)
		return
	}

	machine, err := h.machineRepo.Create(r.Context(), hostID, params, string(hash))
	if err != nil {
		log.Error().Err(err).Msg("machine registration failed")
		http.Error(w, "registration failed", http.StatusInternalServerError)
		return
	}

	resp := model.RegisterMachineResponse{Machine: *machine, AgentToken: plainToken}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(resp)
}

func (h *HostHandler) GetMachine(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	id, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		http.Error(w, "invalid id", http.StatusBadRequest)
		return
	}

	machine, err := h.machineRepo.GetByID(r.Context(), id)
	if err != nil {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if machine.HostID != hostID {
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(machine)
}

func (h *HostHandler) DeleteMachine(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	id, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		http.Error(w, "invalid id", http.StatusBadRequest)
		return
	}
	if err := h.machineRepo.Delete(r.Context(), id, hostID); err != nil {
		http.Error(w, "delete failed", http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *HostHandler) GetEarnings(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	summary, err := h.billingRepo.GetEarningsSummary(r.Context(), hostID)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(summary)
}

func (h *HostHandler) RequestPayout(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	summary, err := h.billingRepo.GetEarningsSummary(r.Context(), hostID)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}

	minPayout := int64(5000)
	if summary.PendingPayoutCents < minPayout {
		http.Error(w, "minimum payout not reached", http.StatusBadRequest)
		return
	}

	payout, err := h.payoutRepo.Create(r.Context(), hostID, summary.PendingPayoutCents, "stripe_connect")
	if err != nil {
		http.Error(w, "payout creation failed", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(payout)
}

func (h *HostHandler) ListPayouts(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	payouts, err := h.payoutRepo.ListByHost(r.Context(), hostID)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(payouts)
}

func (h *HostHandler) Heartbeat(w http.ResponseWriter, r *http.Request) {
	var hb model.MachineHeartbeat
	if err := json.NewDecoder(r.Body).Decode(&hb); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}
	if err := h.machineRepo.UpdateHeartbeat(r.Context(), hb.MachineID, hb); err != nil {
		log.Error().Err(err).Str("machine", hb.MachineID.String()).Msg("heartbeat update failed")
		http.Error(w, "update failed", http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}
