-- Crypto deposit addresses (HD wallet per user)
CREATE TABLE crypto_deposit_addresses (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    chain       TEXT NOT NULL DEFAULT 'base',
    address     TEXT NOT NULL,
    hd_index    INTEGER NOT NULL,
    created_at  TIMESTAMPTZ DEFAULT now(),
    UNIQUE(user_id, chain)
);

CREATE INDEX idx_crypto_deposits_addr ON crypto_deposit_addresses(address);
CREATE INDEX idx_crypto_deposits_user ON crypto_deposit_addresses(user_id);

-- Crypto deposits (incoming token transfers)
CREATE TYPE crypto_tx_status AS ENUM ('pending', 'confirmed', 'credited', 'failed');

CREATE TABLE crypto_deposits (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    chain           TEXT NOT NULL DEFAULT 'base',
    tx_hash         TEXT NOT NULL UNIQUE,
    token_address   TEXT NOT NULL,
    token_symbol    TEXT NOT NULL DEFAULT 'WST',
    amount_wei      TEXT NOT NULL,
    amount_usd_cents BIGINT NOT NULL,
    status          crypto_tx_status DEFAULT 'pending',
    block_number    BIGINT,
    confirmations   INTEGER DEFAULT 0,
    credited_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_crypto_dep_user ON crypto_deposits(user_id);
CREATE INDEX idx_crypto_dep_status ON crypto_deposits(status);
CREATE INDEX idx_crypto_dep_block ON crypto_deposits(block_number);

-- Crypto withdrawals (outgoing token transfers)
CREATE TABLE crypto_withdrawals (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    chain           TEXT NOT NULL DEFAULT 'base',
    to_address      TEXT NOT NULL,
    token_address   TEXT NOT NULL,
    token_symbol    TEXT NOT NULL DEFAULT 'WST',
    amount_wei      TEXT NOT NULL,
    amount_usd_cents BIGINT NOT NULL,
    tx_hash         TEXT,
    status          crypto_tx_status DEFAULT 'pending',
    created_at      TIMESTAMPTZ DEFAULT now(),
    completed_at    TIMESTAMPTZ
);

CREATE INDEX idx_crypto_wd_user ON crypto_withdrawals(user_id);
CREATE INDEX idx_crypto_wd_status ON crypto_withdrawals(status);

-- Payment methods (track how users pay)
CREATE TYPE payment_method_type AS ENUM ('stripe', 'crypto');

CREATE TABLE payment_methods (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    type            payment_method_type NOT NULL,
    stripe_pm_id    TEXT,
    chain           TEXT,
    wallet_address  TEXT,
    label           TEXT DEFAULT '',
    is_default      BOOLEAN DEFAULT FALSE,
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_pm_user ON payment_methods(user_id);

-- Add Stripe Connect fields to host_payout_settings
ALTER TABLE host_payout_settings
    ADD COLUMN IF NOT EXISTS stripe_connect_status TEXT DEFAULT 'not_started',
    ADD COLUMN IF NOT EXISTS stripe_connect_onboarding_url TEXT,
    ADD COLUMN IF NOT EXISTS charges_enabled BOOLEAN DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS payouts_enabled BOOLEAN DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS crypto_wallet_address TEXT;

-- Add crypto wallet to profiles
ALTER TABLE profiles
    ADD COLUMN IF NOT EXISTS crypto_wallet_address TEXT;

-- RLS for new tables
ALTER TABLE crypto_deposit_addresses ENABLE ROW LEVEL SECURITY;
ALTER TABLE crypto_deposits ENABLE ROW LEVEL SECURITY;
ALTER TABLE crypto_withdrawals ENABLE ROW LEVEL SECURITY;
ALTER TABLE payment_methods ENABLE ROW LEVEL SECURITY;

CREATE POLICY crypto_addr_user ON crypto_deposit_addresses FOR ALL USING (auth.uid() = user_id);
CREATE POLICY crypto_dep_user ON crypto_deposits FOR SELECT USING (auth.uid() = user_id);
CREATE POLICY crypto_wd_user ON crypto_withdrawals FOR ALL USING (auth.uid() = user_id);
CREATE POLICY pm_user ON payment_methods FOR ALL USING (auth.uid() = user_id);
