-- Credit Transactions (full ledger)
CREATE TABLE credit_transactions (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id                  UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    amount_cents             BIGINT NOT NULL,
    type                     transaction_type NOT NULL,
    description              TEXT DEFAULT '',
    instance_id              UUID REFERENCES instances(id),
    stripe_payment_intent_id TEXT,
    balance_after_cents      BIGINT NOT NULL,
    created_at               TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_txn_user ON credit_transactions(user_id);
CREATE INDEX idx_txn_instance ON credit_transactions(instance_id);
CREATE INDEX idx_txn_time ON credit_transactions(created_at);

-- Host Earnings
CREATE TABLE host_earnings (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    host_id             UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    instance_id         UUID NOT NULL REFERENCES instances(id),
    amount_cents        BIGINT NOT NULL,
    platform_fee_cents  BIGINT NOT NULL,
    net_amount_cents    BIGINT NOT NULL,
    period_start        TIMESTAMPTZ NOT NULL,
    period_end          TIMESTAMPTZ NOT NULL,
    created_at          TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_earnings_host ON host_earnings(host_id);

-- Payouts
CREATE TABLE payouts (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    host_id             UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    amount_cents        BIGINT NOT NULL,
    status              payout_status DEFAULT 'pending',
    stripe_transfer_id  TEXT,
    payout_method       TEXT DEFAULT '',
    requested_at        TIMESTAMPTZ DEFAULT now(),
    completed_at        TIMESTAMPTZ
);

CREATE INDEX idx_payouts_host ON payouts(host_id);
CREATE INDEX idx_payouts_status ON payouts(status);

-- Host Payout Settings
CREATE TABLE host_payout_settings (
    host_id                    UUID PRIMARY KEY REFERENCES profiles(id) ON DELETE CASCADE,
    stripe_connect_account_id  TEXT,
    payout_threshold_cents     BIGINT DEFAULT 5000,
    auto_payout                BOOLEAN DEFAULT FALSE,
    updated_at                 TIMESTAMPTZ DEFAULT now()
);
