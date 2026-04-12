package handler

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/middleware"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
)

type InstanceHandler struct {
	instanceRepo *repository.InstanceRepo
	machineRepo  *repository.MachineRepo
	billingRepo  *repository.BillingRepo
}

func NewInstanceHandler(ir *repository.InstanceRepo, mr *repository.MachineRepo, br *repository.BillingRepo) *InstanceHandler {
	return &InstanceHandler{instanceRepo: ir, machineRepo: mr, billingRepo: br}
}

func (h *InstanceHandler) List(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	instances, err := h.instanceRepo.ListByUser(r.Context(), userID)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(instances)
}

func (h *InstanceHandler) Create(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	var params model.CreateInstanceParams
	if err := json.NewDecoder(r.Body).Decode(&params); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}

	machine, err := h.machineRepo.GetByID(r.Context(), params.MachineID)
	if err != nil {
		http.Error(w, "machine not found", http.StatusNotFound)
		return
	}
	if !machine.IsAvailable || machine.Status != "online" {
		http.Error(w, "machine not available", http.StatusConflict)
		return
	}

	balance, err := h.billingRepo.GetBalance(r.Context(), userID)
	if err != nil {
		http.Error(w, "billing error", http.StatusInternalServerError)
		return
	}
	if balance < int64(machine.PricePerHourCents) {
		http.Error(w, "insufficient credits", http.StatusPaymentRequired)
		return
	}

	if params.DiskGB <= 0 {
		params.DiskGB = 20
	}

	inst, err := h.instanceRepo.Create(r.Context(), params, userID, machine.HostID, machine.PricePerHourCents)
	if err != nil {
		log.Error().Err(err).Msg("failed to create instance")
		http.Error(w, "creation failed", http.StatusInternalServerError)
		return
	}

	_ = h.instanceRepo.InsertEvent(r.Context(), inst.ID, "created", map[string]string{
		"docker_image": params.DockerImage,
		"machine_id":   params.MachineID.String(),
	})

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(inst)
}

func (h *InstanceHandler) Get(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	id, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		http.Error(w, "invalid id", http.StatusBadRequest)
		return
	}

	inst, err := h.instanceRepo.GetByID(r.Context(), id)
	if err != nil {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if inst.UserID != userID {
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(inst)
}

func (h *InstanceHandler) Stop(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	id, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		http.Error(w, "invalid id", http.StatusBadRequest)
		return
	}

	inst, err := h.instanceRepo.GetByID(r.Context(), id)
	if err != nil {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if inst.UserID != userID {
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	}
	if inst.Status != model.InstanceStatusRunning {
		http.Error(w, "instance is not running", http.StatusConflict)
		return
	}

	if err := h.instanceRepo.UpdateStatus(r.Context(), id, model.InstanceStatusStopping); err != nil {
		http.Error(w, "update failed", http.StatusInternalServerError)
		return
	}
	_ = h.instanceRepo.InsertEvent(r.Context(), id, "stopping", nil)

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{"status": "stopping"})
}

func (h *InstanceHandler) Start(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	id, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		http.Error(w, "invalid id", http.StatusBadRequest)
		return
	}

	inst, err := h.instanceRepo.GetByID(r.Context(), id)
	if err != nil {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if inst.UserID != userID {
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	}
	if inst.Status != model.InstanceStatusStopped {
		http.Error(w, "instance is not stopped", http.StatusConflict)
		return
	}

	balance, _ := h.billingRepo.GetBalance(r.Context(), userID)
	if balance < int64(inst.PricePerHourCents) {
		http.Error(w, "insufficient credits", http.StatusPaymentRequired)
		return
	}

	if err := h.instanceRepo.UpdateStatus(r.Context(), id, model.InstanceStatusCreating); err != nil {
		http.Error(w, "update failed", http.StatusInternalServerError)
		return
	}
	_ = h.instanceRepo.InsertEvent(r.Context(), id, "starting", nil)

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{"status": "starting"})
}

func (h *InstanceHandler) Destroy(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	id, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		http.Error(w, "invalid id", http.StatusBadRequest)
		return
	}

	inst, err := h.instanceRepo.GetByID(r.Context(), id)
	if err != nil {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if inst.UserID != userID {
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	}
	if inst.Status == model.InstanceStatusDestroyed {
		http.Error(w, "already destroyed", http.StatusConflict)
		return
	}

	if err := h.instanceRepo.UpdateStatus(r.Context(), id, model.InstanceStatusDestroying); err != nil {
		http.Error(w, "update failed", http.StatusInternalServerError)
		return
	}
	_ = h.instanceRepo.InsertEvent(r.Context(), id, "destroying", nil)

	w.WriteHeader(http.StatusAccepted)
	json.NewEncoder(w).Encode(map[string]string{"status": "destroying"})
}
