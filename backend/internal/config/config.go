package config

import (
	"github.com/caarlos0/env/v9"
)

type Config struct {
	ServerPort                string `env:"SERVER_PORT" envDefault:"8080"`
	DatabaseURL               string `env:"DATABASE_URL,required"`
	SupabaseURL               string `env:"SUPABASE_URL,required"`
	SupabaseAnonKey           string `env:"SUPABASE_ANON_KEY,required"`
	SupabaseServiceRoleKey    string `env:"SUPABASE_SERVICE_ROLE_KEY,required"`
	SupabaseJWTSecret         string `env:"SUPABASE_JWT_SECRET,required"`
	StripeSecretKey           string `env:"STRIPE_SECRET_KEY,required"`
	StripeWebhookSecret       string `env:"STRIPE_WEBHOOK_SECRET,required"`
	StripeConnectWebhookSecret string `env:"STRIPE_CONNECT_WEBHOOK_SECRET" envDefault:""`
	StripeConnectReturnURL    string `env:"STRIPE_CONNECT_RETURN_URL" envDefault:"http://localhost:3000/host/connect/return"`
	StripeConnectRefreshURL   string `env:"STRIPE_CONNECT_REFRESH_URL" envDefault:"http://localhost:3000/host/connect/refresh"`
	CORSOrigins               string `env:"CORS_ORIGINS" envDefault:"http://localhost:3000"`
	AgentHeartbeatStaleSec    int    `env:"AGENT_HEARTBEAT_STALE_SECONDS" envDefault:"120"`
	BaseRPCURL                string `env:"BASE_RPC_URL" envDefault:"https://mainnet.base.org"`
	WSTTokenAddress           string `env:"WST_TOKEN_ADDRESS" envDefault:""`
	PlatformWalletAddress     string `env:"PLATFORM_WALLET_ADDRESS" envDefault:""`
	PlatformWalletPrivateKey  string `env:"PLATFORM_WALLET_PRIVATE_KEY" envDefault:""`
	DepositHDSeed             string `env:"DEPOSIT_HD_SEED" envDefault:""`
	WSTPriceUSDCents          int64  `env:"WST_PRICE_USD_CENTS" envDefault:"100"`
	DepositConfirmations      int    `env:"DEPOSIT_CONFIRMATIONS_REQUIRED" envDefault:"12"`
}

func Load() (*Config, error) {
	cfg := &Config{}
	if err := env.Parse(cfg); err != nil {
		return nil, err
	}
	return cfg, nil
}
