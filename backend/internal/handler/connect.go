package handler

import (
	"encoding/json"
	"io"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
	"github.com/stripe/stripe-go/v76"
	"github.com/stripe/stripe-go/v76/account"
	"github.com/stripe/stripe-go/v76/accountlink"
	stripe_transfer "github.com/stripe/stripe-go/v76/transfer"
	"github.com/stripe/stripe-go/v76/webhook"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/middleware"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
)

type ConnectHandler struct {
	payoutRepo      *repository.PayoutRepo
	billingRepo     *repository.BillingRepo
	stripeKey       string
	webhookSecret   string
	returnURL       string
	refreshURL      string
}

func NewConnectHandler(pr *repository.PayoutRepo, br *repository.BillingRepo, stripeKey, webhookSecret, returnURL, refreshURL string) *ConnectHandler {
	return &ConnectHandler{
		payoutRepo:    pr,
		billingRepo:   br,
		stripeKey:     stripeKey,
		webhookSecret: webhookSecret,
		returnURL:     returnURL,
		refreshURL:    refreshURL,
	}
}

func (h *ConnectHandler) Onboard(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	stripe.Key = h.stripeKey

	settings, err := h.payoutRepo.GetSettings(r.Context(), hostID)
	if err != nil {
		settings = &model.PayoutSettings{HostID: hostID, PayoutThresholdCents: 5000}
	}

	var acctID string
	if settings.StripeConnectAccountID != nil && *settings.StripeConnectAccountID != "" {
		acctID = *settings.StripeConnectAccountID
	} else {
		acctParams := &stripe.AccountParams{
			Type: stripe.String(string(stripe.AccountTypeExpress)),
			Capabilities: &stripe.AccountCapabilitiesParams{
				Transfers: &stripe.AccountCapabilitiesTransfersParams{
					Requested: stripe.Bool(true),
				},
			},
			Metadata: map[string]string{
				"host_id": hostID.String(),
			},
		}

		acct, err := account.New(acctParams)
		if err != nil {
			log.Error().Err(err).Msg("failed to create connect account")
			http.Error(w, "account creation failed", http.StatusInternalServerError)
			return
		}
		acctID = acct.ID

		settings.StripeConnectAccountID = &acctID
		_ = h.payoutRepo.UpsertSettings(r.Context(), *settings)
	}

	linkParams := &stripe.AccountLinkParams{
		Account:    stripe.String(acctID),
		RefreshURL: stripe.String(h.refreshURL),
		ReturnURL:  stripe.String(h.returnURL),
		Type:       stripe.String("account_onboarding"),
	}

	link, err := accountlink.New(linkParams)
	if err != nil {
		log.Error().Err(err).Msg("failed to create account link")
		http.Error(w, "link creation failed", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{
		"onboarding_url": link.URL,
		"account_id":     acctID,
	})
}

func (h *ConnectHandler) Status(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	stripe.Key = h.stripeKey

	settings, err := h.payoutRepo.GetSettings(r.Context(), hostID)
	if err != nil || settings.StripeConnectAccountID == nil {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"status":          "not_started",
			"charges_enabled": false,
			"payouts_enabled": false,
		})
		return
	}

	acct, err := account.GetByID(*settings.StripeConnectAccountID, nil)
	if err != nil {
		http.Error(w, "account fetch failed", http.StatusInternalServerError)
		return
	}

	status := "pending"
	if acct.ChargesEnabled && acct.PayoutsEnabled {
		status = "active"
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]interface{}{
		"status":          status,
		"account_id":      acct.ID,
		"charges_enabled": acct.ChargesEnabled,
		"payouts_enabled": acct.PayoutsEnabled,
		"details_submitted": acct.DetailsSubmitted,
	})
}

func (h *ConnectHandler) ExecutePayout(w http.ResponseWriter, r *http.Request) {
	hostID := middleware.GetUserID(r.Context())
	payoutID, err := uuid.Parse(chi.URLParam(r, "payoutID"))
	if err != nil {
		http.Error(w, "invalid payout id", http.StatusBadRequest)
		return
	}

	stripe.Key = h.stripeKey

	settings, err := h.payoutRepo.GetSettings(r.Context(), hostID)
	if err != nil || settings.StripeConnectAccountID == nil {
		http.Error(w, "no connect account configured", http.StatusBadRequest)
		return
	}

	payouts, err := h.payoutRepo.ListByHost(r.Context(), hostID)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}

	var targetPayout *model.Payout
	for i := range payouts {
		if payouts[i].ID == payoutID && payouts[i].Status == model.PayoutPending {
			targetPayout = &payouts[i]
			break
		}
	}
	if targetPayout == nil {
		http.Error(w, "payout not found or not pending", http.StatusNotFound)
		return
	}

	_ = h.payoutRepo.UpdateStatus(r.Context(), payoutID, model.PayoutProcessing, nil)

	transferParams := &stripe.TransferParams{
		Amount:      stripe.Int64(targetPayout.AmountCents),
		Currency:    stripe.String("usd"),
		Destination: stripe.String(*settings.StripeConnectAccountID),
		Metadata: map[string]string{
			"payout_id": payoutID.String(),
			"host_id":   hostID.String(),
		},
	}

	transfer, err := stripe_transfer.New(transferParams)
	if err != nil {
		log.Error().Err(err).Str("payout", payoutID.String()).Msg("stripe transfer failed")
		_ = h.payoutRepo.UpdateStatus(r.Context(), payoutID, model.PayoutFailed, nil)
		http.Error(w, "transfer failed: "+err.Error(), http.StatusInternalServerError)
		return
	}

	transferID := transfer.ID
	_ = h.payoutRepo.UpdateStatus(r.Context(), payoutID, model.PayoutCompleted, &transferID)

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{
		"status":      "completed",
		"transfer_id": transfer.ID,
	})
}

func (h *ConnectHandler) ConnectWebhook(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "read failed", http.StatusBadRequest)
		return
	}

	event, err := webhook.ConstructEvent(body, r.Header.Get("Stripe-Signature"), h.webhookSecret)
	if err != nil {
		http.Error(w, "invalid signature", http.StatusBadRequest)
		return
	}

	switch event.Type {
	case "account.updated":
		var acct stripe.Account
		if err := json.Unmarshal(event.Data.Raw, &acct); err != nil {
			log.Error().Err(err).Msg("failed to parse account.updated")
			break
		}
		hostIDStr := acct.Metadata["host_id"]
		if hostIDStr == "" {
			break
		}
		hostID, err := uuid.Parse(hostIDStr)
		if err != nil {
			break
		}

		settings, err := h.payoutRepo.GetSettings(r.Context(), hostID)
		if err != nil {
			settings = &model.PayoutSettings{HostID: hostID, PayoutThresholdCents: 5000}
			settings.StripeConnectAccountID = &acct.ID
		}

		_ = h.payoutRepo.UpdateConnectStatus(r.Context(), hostID, acct.ChargesEnabled, acct.PayoutsEnabled)

		log.Info().Str("host", hostID.String()).Bool("charges", acct.ChargesEnabled).Bool("payouts", acct.PayoutsEnabled).Msg("connect account updated")

	case "transfer.created", "transfer.updated":
		log.Info().Str("event", string(event.Type)).Msg("transfer event received")
	}

	w.WriteHeader(http.StatusOK)
}
