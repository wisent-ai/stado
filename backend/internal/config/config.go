package config

import (
	"github.com/caarlos0/env/v9"
)

type Config struct {
	ServerPort             string `env:"SERVER_PORT" envDefault:"8080"`
	DatabaseURL            string `env:"DATABASE_URL,required"`
	SupabaseURL            string `env:"SUPABASE_URL,required"`
	SupabaseAnonKey        string `env:"SUPABASE_ANON_KEY,required"`
	SupabaseServiceRoleKey string `env:"SUPABASE_SERVICE_ROLE_KEY,required"`
	SupabaseJWTSecret      string `env:"SUPABASE_JWT_SECRET,required"`
	StripeSecretKey        string `env:"STRIPE_SECRET_KEY,required"`
	StripeWebhookSecret    string `env:"STRIPE_WEBHOOK_SECRET,required"`
	CORSOrigins            string `env:"CORS_ORIGINS" envDefault:"http://localhost:3000"`
	AgentHeartbeatStaleSec int    `env:"AGENT_HEARTBEAT_STALE_SECONDS" envDefault:"120"`
}

func Load() (*Config, error) {
	cfg := &Config{}
	if err := env.Parse(cfg); err != nil {
		return nil, err
	}
	return cfg, nil
}
