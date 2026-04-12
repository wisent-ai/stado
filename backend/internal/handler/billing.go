package handler

import (
	"encoding/json"
	"io"
	"net/http"
	"strconv"

	"github.com/rs/zerolog/log"
	"github.com/stripe/stripe-go/v76"
	checkoutsession "github.com/stripe/stripe-go/v76/checkout/session"
	"github.com/stripe/stripe-go/v76/webhook"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/middleware"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
)

type BillingHandler struct {
	billingRepo     *repository.BillingRepo
	webhookSecret   string
	stripeKey       string
	successURL      string
	cancelURL       string
}

func NewBillingHandler(br *repository.BillingRepo, webhookSecret, stripeKey, corsOrigins string) *BillingHandler {
	return &BillingHandler{
		billingRepo:   br,
		webhookSecret: webhookSecret,
		stripeKey:     stripeKey,
		successURL:    corsOrigins + "/billing?success=true",
		cancelURL:     corsOrigins + "/billing?canceled=true",
	}
}

func (h *BillingHandler) GetBalance(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	balance, err := h.billingRepo.GetBalance(r.Context(), userID)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]interface{}{
		"balance_cents": balance,
		"packages":      model.CreditPackages,
	})
}

func (h *BillingHandler) ListTransactions(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	limit := 50
	offset := 0
	if v, err := strconv.Atoi(r.URL.Query().Get("limit")); err == nil && v > 0 {
		limit = v
	}
	if v, err := strconv.Atoi(r.URL.Query().Get("offset")); err == nil && v >= 0 {
		offset = v
	}

	txns, err := h.billingRepo.ListTransactions(r.Context(), userID, limit, offset)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(txns)
}

func (h *BillingHandler) Checkout(w http.ResponseWriter, r *http.Request) {
	userID := middleware.GetUserID(r.Context())
	var req model.CheckoutRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}

	var pkg *model.CreditPackage
	for i := range model.CreditPackages {
		if model.CreditPackages[i].ID == req.PackageID {
			pkg = &model.CreditPackages[i]
			break
		}
	}
	if pkg == nil {
		http.Error(w, "invalid package", http.StatusBadRequest)
		return
	}

	stripe.Key = h.stripeKey
	params := &stripe.CheckoutSessionParams{
		Mode: stripe.String(string(stripe.CheckoutSessionModePayment)),
		LineItems: []*stripe.CheckoutSessionLineItemParams{{
			PriceData: &stripe.CheckoutSessionLineItemPriceDataParams{
				Currency:   stripe.String("usd"),
				UnitAmount: stripe.Int64(pkg.PriceCents),
				ProductData: &stripe.CheckoutSessionLineItemPriceDataProductDataParams{
					Name: stripe.String(pkg.Name + " Credits"),
				},
			},
			Quantity: stripe.Int64(1),
		}},
		SuccessURL: stripe.String(h.successURL),
		CancelURL:  stripe.String(h.cancelURL),
		Metadata: map[string]string{
			"user_id":    userID.String(),
			"package_id": pkg.ID,
			"credits":    strconv.FormatInt(pkg.CreditsCents, 10),
		},
	}

	session, err := checkoutsession.New(params)
	if err != nil {
		log.Error().Err(err).Msg("stripe checkout session creation failed")
		http.Error(w, "checkout failed", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(model.CheckoutResponse{CheckoutURL: session.URL})
}

func (h *BillingHandler) StripeWebhook(w http.ResponseWriter, r *http.Request) {
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

	if event.Type == "checkout.session.completed" {
		var session stripe.CheckoutSession
		if err := json.Unmarshal(event.Data.Raw, &session); err != nil {
			http.Error(w, "parse failed", http.StatusBadRequest)
			return
		}

		userIDStr := session.Metadata["user_id"]
		creditsStr := session.Metadata["credits"]
		userID, _ := parseUUID(userIDStr)
		credits, _ := strconv.ParseInt(creditsStr, 10, 64)

		piID := ""
		if session.PaymentIntent != nil {
			piID = session.PaymentIntent.ID
		}

		_, err = h.billingRepo.AddCredits(r.Context(), userID, credits,
			model.TransactionPurchase, "Credit purchase: "+session.Metadata["package_id"], &piID)
		if err != nil {
			log.Error().Err(err).Msg("failed to add credits")
		}
	}

	w.WriteHeader(http.StatusOK)
}

func parseUUID(s string) (uuid.UUID, error) {
	return uuid.Parse(s)
}

import "github.com/google/uuid"
