package main

import (
	"context"
	"net/http"
	"os"
	"os/signal"
	"syscall"

	"github.com/go-chi/chi/v5"
	chimw "github.com/go-chi/chi/v5/middleware"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/rs/zerolog"
	"github.com/rs/zerolog/log"

	"github.com/wisent-ai/compute.wisent.com/backend/internal/config"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/handler"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/middleware"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/scheduler"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/ws"
)

func main() {
	zerolog.TimeFieldFormat = zerolog.TimeFormatUnix
	log.Logger = log.Output(zerolog.ConsoleWriter{Out: os.Stderr})

	cfg, err := config.Load()
	if err != nil {
		log.Fatal().Err(err).Msg("failed to load config")
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	pool, err := pgxpool.New(ctx, cfg.DatabaseURL)
	if err != nil {
		log.Fatal().Err(err).Msg("failed to connect to database")
	}
	defer pool.Close()

	if err := pool.Ping(ctx); err != nil {
		log.Fatal().Err(err).Msg("database ping failed")
	}
	log.Info().Msg("connected to database")

	// Repositories
	userRepo := repository.NewUserRepo(pool)
	machineRepo := repository.NewMachineRepo(pool)
	instanceRepo := repository.NewInstanceRepo(pool)
	billingRepo := repository.NewBillingRepo(pool)
	payoutRepo := repository.NewPayoutRepo(pool)
	cryptoRepo := repository.NewCryptoRepo(pool)

	// Handlers
	healthH := handler.NewHealthHandler()
	userH := handler.NewUserHandler(userRepo, pool)
	offerH := handler.NewOfferHandler(machineRepo)
	instanceH := handler.NewInstanceHandler(instanceRepo, machineRepo, billingRepo)
	billingH := handler.NewBillingHandler(billingRepo, cfg.StripeWebhookSecret, cfg.StripeSecretKey, cfg.CORSOrigins)
	hostH := handler.NewHostHandler(machineRepo, billingRepo, payoutRepo, userRepo, instanceRepo)
	connectH := handler.NewConnectHandler(payoutRepo, billingRepo, cfg.StripeSecretKey, cfg.StripeConnectWebhookSecret, cfg.StripeConnectReturnURL, cfg.StripeConnectRefreshURL)
	cryptoH := handler.NewCryptoHandler(cryptoRepo, billingRepo, cfg, nil)

	hub := ws.NewHub()

	// Router
	r := chi.NewRouter()
	r.Use(chimw.RequestID)
	r.Use(chimw.RealIP)
	r.Use(middleware.Logger)
	r.Use(middleware.CORSHandler(cfg.CORSOrigins).Handler)
	r.Use(chimw.Recoverer)

	// Public
	r.Get("/healthz", healthH.Health)
	r.Post("/api/v1/auth/callback", userH.AuthCallback)
	r.Get("/api/v1/offers", offerH.List)
	r.Get("/api/v1/offers/{id}", offerH.Get)
	r.Get("/api/v1/crypto/price", cryptoH.GetPrice)

	// Webhooks (signature-verified, no JWT)
	r.Post("/api/v1/billing/webhook/stripe", billingH.StripeWebhook)
	r.Post("/api/v1/billing/webhook/stripe-connect", connectH.ConnectWebhook)

	// Agent endpoints (agent token auth)
	r.Post("/api/v1/agent/heartbeat", hostH.Heartbeat)

	// Authenticated endpoints
	r.Group(func(r chi.Router) {
		r.Use(middleware.Auth(cfg.SupabaseJWTSecret, pool))

		// User
		r.Get("/api/v1/users/me", userH.GetMe)
		r.Put("/api/v1/users/me", userH.UpdateMe)

		// Instances
		r.Get("/api/v1/instances", instanceH.List)
		r.Post("/api/v1/instances", instanceH.Create)
		r.Get("/api/v1/instances/{id}", instanceH.Get)
		r.Post("/api/v1/instances/{id}/start", instanceH.Start)
		r.Post("/api/v1/instances/{id}/stop", instanceH.Stop)
		r.Delete("/api/v1/instances/{id}", instanceH.Destroy)

		// Billing (Stripe)
		r.Get("/api/v1/billing/balance", billingH.GetBalance)
		r.Get("/api/v1/billing/transactions", billingH.ListTransactions)
		r.Post("/api/v1/billing/checkout", billingH.Checkout)

		// Crypto (WST token)
		r.Get("/api/v1/crypto/deposit-address", cryptoH.GetDepositAddress)
		r.Post("/api/v1/crypto/withdraw", cryptoH.Withdraw)
		r.Get("/api/v1/crypto/transactions", cryptoH.ListTransactions)

		// API Keys
		r.Get("/api/v1/api-keys", userH.ListAPIKeys)
		r.Post("/api/v1/api-keys", userH.CreateAPIKey)
		r.Delete("/api/v1/api-keys/{keyID}", userH.RevokeAPIKey)

		// Host endpoints
		r.Route("/api/v1/host", func(r chi.Router) {
			r.Use(middleware.RequireHost)
			r.Get("/machines", hostH.ListMachines)
			r.Post("/machines", hostH.RegisterMachine)
			r.Get("/machines/{id}", hostH.GetMachine)
			r.Delete("/machines/{id}", hostH.DeleteMachine)
			r.Get("/earnings", hostH.GetEarnings)
			r.Post("/payouts", hostH.RequestPayout)
			r.Get("/payouts", hostH.ListPayouts)
			// Stripe Connect
			r.Post("/connect/onboard", connectH.Onboard)
			r.Get("/connect/status", connectH.Status)
			r.Post("/payouts/{payoutID}/execute", connectH.ExecutePayout)
		})

		// WebSocket
		r.Get("/api/v1/ws", func(w http.ResponseWriter, r *http.Request) {
			ws.ServeWS(hub, cfg.SupabaseJWTSecret, w, r)
		})
	})

	// Background schedulers
	billingTicker := scheduler.NewBillingTicker(instanceRepo, billingRepo)
	go billingTicker.Start(ctx)

	staleChecker := scheduler.NewStaleChecker(machineRepo, cfg.AgentHeartbeatStaleSec)
	go staleChecker.Start(ctx)

	depositMonitor := scheduler.NewDepositMonitor(cryptoRepo, billingRepo, cfg)
	go depositMonitor.Start(ctx)

	// Start server
	log.Info().Str("port", cfg.ServerPort).Msg("starting server")
	server := &http.Server{Addr: ":" + cfg.ServerPort, Handler: r}

	go func() {
		if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatal().Err(err).Msg("server failed")
		}
	}()

	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)
	<-quit

	log.Info().Msg("shutting down server")
	cancel()
	server.Shutdown(context.Background())
}
