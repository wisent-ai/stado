package handler

import (
	"encoding/json"
	"net/http"
	"strconv"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
)

type OfferHandler struct {
	machineRepo *repository.MachineRepo
}

func NewOfferHandler(machineRepo *repository.MachineRepo) *OfferHandler {
	return &OfferHandler{machineRepo: machineRepo}
}

func (h *OfferHandler) List(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()

	filter := repository.ListOffersFilter{
		Country: q.Get("country"),
		SortBy:  q.Get("sort_by"),
		SortDir: q.Get("sort_dir"),
	}

	if v := q.Get("gpu_model_ids"); v != "" {
		for _, s := range strings.Split(v, ",") {
			if id, err := strconv.Atoi(s); err == nil {
				filter.GPUModelIDs = append(filter.GPUModelIDs, id)
			}
		}
	}
	if v, err := strconv.Atoi(q.Get("min_vram_gb")); err == nil {
		filter.MinVRAMGB = v
	}
	if v, err := strconv.Atoi(q.Get("max_price_cents")); err == nil {
		filter.MaxPriceCents = v
	}
	if v, err := strconv.Atoi(q.Get("min_price_cents")); err == nil {
		filter.MinPriceCents = v
	}
	if v, err := strconv.Atoi(q.Get("limit")); err == nil {
		filter.Limit = v
	}
	if v, err := strconv.Atoi(q.Get("offset")); err == nil {
		filter.Offset = v
	}

	machines, total, err := h.machineRepo.ListOffers(r.Context(), filter)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]interface{}{
		"offers": machines,
		"total":  total,
		"limit":  filter.Limit,
		"offset": filter.Offset,
	})
}

func (h *OfferHandler) Get(w http.ResponseWriter, r *http.Request) {
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

	if !machine.IsAvailable || machine.Status != "online" {
		http.Error(w, "not available", http.StatusNotFound)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(machine)
}
