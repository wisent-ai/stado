-- Machines (host-registered GPU machines)
CREATE TABLE machines (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    host_id             UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    hostname            TEXT DEFAULT '',
    label               TEXT DEFAULT '',
    status              machine_status DEFAULT 'pending',
    gpu_model_id        INTEGER REFERENCES gpu_models(id),
    gpu_count           INTEGER NOT NULL DEFAULT 1,
    gpu_ram_gb          INTEGER DEFAULT 0,
    cpu_model           TEXT DEFAULT '',
    cpu_cores           INTEGER DEFAULT 0,
    ram_gb              INTEGER DEFAULT 0,
    disk_gb             INTEGER DEFAULT 0,
    disk_type           TEXT DEFAULT '',
    upload_mbps         REAL DEFAULT 0,
    download_mbps       REAL DEFAULT 0,
    country             TEXT DEFAULT '',
    region              TEXT DEFAULT '',
    ip_address          INET,
    cuda_version        TEXT DEFAULT '',
    docker_version      TEXT DEFAULT '',
    os_version          TEXT DEFAULT '',
    driver_version      TEXT DEFAULT '',
    price_per_hour_cents INTEGER NOT NULL DEFAULT 0,
    min_rental_hours    INTEGER DEFAULT 1,
    max_rental_hours    INTEGER,
    agent_version       TEXT DEFAULT '',
    agent_token_hash    TEXT,
    last_heartbeat      TIMESTAMPTZ,
    uptime_percentage   REAL DEFAULT 100.0,
    total_rentals       INTEGER DEFAULT 0,
    avg_rating          REAL,
    is_available        BOOLEAN DEFAULT FALSE,
    current_instance_id UUID,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_machines_host ON machines(host_id);
CREATE INDEX idx_machines_status ON machines(status);
CREATE INDEX idx_machines_gpu ON machines(gpu_model_id);
CREATE INDEX idx_machines_available ON machines(is_available, status) WHERE is_available = TRUE AND status = 'online';
CREATE INDEX idx_machines_price ON machines(price_per_hour_cents);

-- Instances (rental of a machine by a user)
CREATE TABLE instances (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id             UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    machine_id          UUID NOT NULL REFERENCES machines(id),
    host_id             UUID NOT NULL REFERENCES profiles(id),
    status              instance_status DEFAULT 'creating',
    docker_image        TEXT NOT NULL,
    docker_env          JSONB DEFAULT '{}',
    docker_ports        JSONB DEFAULT '[]',
    docker_cmd          TEXT,
    docker_args         JSONB DEFAULT '{}',
    disk_gb             INTEGER DEFAULT 20,
    ssh_host            TEXT DEFAULT '',
    ssh_port            INTEGER DEFAULT 0,
    ssh_public_key      TEXT DEFAULT '',
    jupyter_url         TEXT DEFAULT '',
    jupyter_token       TEXT DEFAULT '',
    price_per_hour_cents INTEGER NOT NULL,
    total_cost_cents    BIGINT DEFAULT 0,
    started_at          TIMESTAMPTZ,
    stopped_at          TIMESTAMPTZ,
    destroyed_at        TIMESTAMPTZ,
    last_billed_at      TIMESTAMPTZ,
    gpu_utilization     REAL DEFAULT 0,
    gpu_memory_used_mb  INTEGER DEFAULT 0,
    cpu_utilization     REAL DEFAULT 0,
    ram_used_mb         INTEGER DEFAULT 0,
    label               TEXT DEFAULT '',
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_instances_user ON instances(user_id);
CREATE INDEX idx_instances_machine ON instances(machine_id);
CREATE INDEX idx_instances_host ON instances(host_id);
CREATE INDEX idx_instances_status ON instances(status);
CREATE INDEX idx_instances_active ON instances(status) WHERE status IN ('creating', 'running');

-- Machine Heartbeats (time-series metrics)
CREATE TABLE machine_heartbeats (
    id                  BIGSERIAL PRIMARY KEY,
    machine_id          UUID NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    gpu_utilization     REAL,
    gpu_temp_c          REAL,
    gpu_memory_used_mb  INTEGER,
    gpu_memory_total_mb INTEGER,
    cpu_utilization     REAL,
    ram_used_mb         INTEGER,
    ram_total_mb        INTEGER,
    disk_used_gb        INTEGER,
    disk_total_gb       INTEGER,
    upload_mbps         REAL,
    download_mbps       REAL,
    created_at          TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_heartbeats_machine ON machine_heartbeats(machine_id);
CREATE INDEX idx_heartbeats_time ON machine_heartbeats(created_at);

-- Instance Events (audit log)
CREATE TABLE instance_events (
    id              BIGSERIAL PRIMARY KEY,
    instance_id     UUID NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    event_type      TEXT NOT NULL,
    details         JSONB DEFAULT '{}',
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_instance_events ON instance_events(instance_id);

-- Updated-at trigger
CREATE OR REPLACE FUNCTION update_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER set_updated_at_profiles BEFORE UPDATE ON profiles FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER set_updated_at_machines BEFORE UPDATE ON machines FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER set_updated_at_instances BEFORE UPDATE ON instances FOR EACH ROW EXECUTE FUNCTION update_updated_at();
