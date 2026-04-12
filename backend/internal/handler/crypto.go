package handler

import (
	"crypto/ecdsa"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"

	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/config"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/middleware"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
)

type CryptoHandler struct {
	cryptoRepo  *repository.CryptoRepo
	billingRepo *repository.BillingRepo
	cfg         *config.Config
	hdDeriver   HDDeriver
}

type HDDeriver interface {
	DeriveAddress(seed string, index int) (string, *ecdsa.PrivateKey, error)
}

func NewCryptoHandler(cr *repository.CryptoRepo, br *repository.BillingRepo, cfg *config.Config, hd HDDeriver) *CryptoHandler {
	return &CryptoHandler{cryptoRepo: cr, billingRepo: br, cfg: cfg, hdDeriver: hd}
}

func (h *CryptoHandler) GetDepositAddress(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	chain := "base"

	addr, err := h.cryptoRepo.GetDepositAddress(r.Context(), userID, chain)
	if err == nil {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"address": addr.Address,
			"chain":   addr.Chain,
			"token":   "WST",
		})
		return
	}

	idx, err := h.cryptoRepo.GetNextHDIndex(r.Context(), chain)
	if err != nil {
		http.Error(w, "index generation failed", http.StatusInternalServerError)
		return
	}

	address, _, err := h.hdDeriver.DeriveAddress(h.cfg.DepositHDSeed, idx)
	if err != nil {
		log.Error().Err(err).Msg("HD derivation failed")
		http.Error(w, "address generation failed", http.StatusInternalServerError)
		return
	}

	addr, err = h.cryptoRepo.CreateDepositAddress(r.Context(), userID, chain, address, idx)
	if err != nil {
		http.Error(w, "address storage failed", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]interface{}{
		"address": addr.Address,
		"chain":   addr.Chain,
		"token":   "WST",
	})
}

func (h *CryptoHandler) Withdraw(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	var req model.WithdrawRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}

	if req.AmountCents < 100 {
		http.Error(w, "minimum withdrawal is $1.00", http.StatusBadRequest)
		return
	}

	balance, err := h.billingRepo.GetBalance(r.Context(), userID)
	if err != nil {
		http.Error(w, "balance check failed", http.StatusInternalServerError)
		return
	}
	if balance < req.AmountCents {
		http.Error(w, "insufficient credits", http.StatusPaymentRequired)
		return
	}

	tokenAmount := new(big.Int)
	tokenAmount.SetInt64(req.AmountCents)
	weiPerCent := new(big.Int)
	weiPerCent.Exp(big.NewInt(10), big.NewInt(16), nil)
	tokenAmount.Mul(tokenAmount, weiPerCent)

	wd := model.CryptoWithdrawal{
		UserID:         userID,
		Chain:          "base",
		ToAddress:      req.ToAddress,
		TokenAddress:   h.cfg.WSTTokenAddress,
		TokenSymbol:    "WST",
		AmountWei:      tokenAmount.String(),
		AmountUSDCents: req.AmountCents,
	}

	var instID *uuid.UUID
	desc := fmt.Sprintf("WST withdrawal to %s", req.ToAddress[:10]+"...")
	success, err := h.billingRepo.DeductCredits(r.Context(), userID, req.AmountCents, desc, instID)
	if err != nil || !success {
		http.Error(w, "deduction failed", http.StatusInternalServerError)
		return
	}

	result, err := h.cryptoRepo.InsertWithdrawal(r.Context(), wd)
	if err != nil {
		log.Error().Err(err).Msg("withdrawal insert failed")
		http.Error(w, "withdrawal creation failed", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(result)
}

func (h *CryptoHandler) ListTransactions(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())

	deposits, _ := h.cryptoRepo.ListUserDeposits(r.Context(), userID)
	withdrawals, _ := h.cryptoRepo.ListUserWithdrawals(r.Context(), userID)

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]interface{}{
		"deposits":    deposits,
		"withdrawals": withdrawals,
	})
}

func (h *CryptoHandler) GetPrice(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(model.CryptoPrice{
		Token:         h.cfg.WSTTokenAddress,
		Symbol:        "WST",
		Chain:         "base",
		PriceUSDCents: h.cfg.WSTPriceUSDCents,
	})
}
