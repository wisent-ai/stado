-- Enum types
CREATE TYPE user_role AS ENUM ('user', 'host', 'admin');
CREATE TYPE machine_status AS ENUM ('pending', 'online', 'offline', 'maintenance', 'banned');
CREATE TYPE instance_status AS ENUM ('creating', 'running', 'stopping', 'stopped', 'destroying', 'destroyed', 'error');
CREATE TYPE transaction_type AS ENUM ('purchase', 'rental_charge', 'rental_refund', 'payout', 'bonus', 'adjustment');
CREATE TYPE payout_status AS ENUM ('pending', 'processing', 'completed', 'failed');

-- Profiles (extends auth.users)
CREATE TABLE profiles (
    id                   UUID PRIMARY KEY REFERENCES auth.users(id) ON DELETE CASCADE,
    email                TEXT,
    full_name            TEXT DEFAULT '',
    avatar_url           TEXT DEFAULT '',
    role                 user_role DEFAULT 'user',
    stripe_customer_id   TEXT,
    credit_balance_cents BIGINT DEFAULT 0,
    is_host              BOOLEAN DEFAULT FALSE,
    created_at           TIMESTAMPTZ DEFAULT now(),
    updated_at           TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_profiles_email ON profiles(email);

-- API Keys
CREATE TABLE api_keys (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    key_prefix  TEXT NOT NULL,
    key_hash    TEXT NOT NULL,
    last_used_at TIMESTAMPTZ,
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    revoked_at  TIMESTAMPTZ
);

CREATE INDEX idx_api_keys_user ON api_keys(user_id);
CREATE INDEX idx_api_keys_prefix ON api_keys(key_prefix);

-- GPU Models (reference table)
CREATE TABLE gpu_models (
    id                    SERIAL PRIMARY KEY,
    name                  TEXT NOT NULL UNIQUE,
    manufacturer          TEXT NOT NULL,
    vram_gb               INTEGER NOT NULL,
    architecture          TEXT,
    fp16_tflops           REAL,
    fp32_tflops           REAL,
    memory_bandwidth_gbps REAL,
    tdp_watts             INTEGER,
    created_at            TIMESTAMPTZ DEFAULT now()
);

-- Seed GPU models
INSERT INTO gpu_models (name, manufacturer, vram_gb, architecture, fp16_tflops, fp32_tflops, memory_bandwidth_gbps, tdp_watts) VALUES
('RTX 3090', 'NVIDIA', 24, 'Ampere', 35.6, 35.6, 936, 350),
('RTX 4090', 'NVIDIA', 24, 'Ada Lovelace', 82.6, 82.6, 1008, 450),
('A100 40GB', 'NVIDIA', 40, 'Ampere', 312, 19.5, 1555, 250),
('A100 80GB', 'NVIDIA', 80, 'Ampere', 312, 19.5, 2039, 300),
('H100 SXM', 'NVIDIA', 80, 'Hopper', 989, 67, 3350, 700),
('H100 PCIe', 'NVIDIA', 80, 'Hopper', 756, 51, 2039, 350),
('L40S', 'NVIDIA', 48, 'Ada Lovelace', 183, 91.6, 864, 350),
('A10', 'NVIDIA', 24, 'Ampere', 31.2, 31.2, 600, 150),
('RTX 3080', 'NVIDIA', 10, 'Ampere', 29.8, 29.8, 760, 320),
('RTX 4080', 'NVIDIA', 16, 'Ada Lovelace', 48.7, 48.7, 717, 320);
