-- Atomic credit deduction with ledger entry
CREATE OR REPLACE FUNCTION deduct_credits(
    p_user_id UUID,
    p_amount_cents BIGINT,
    p_description TEXT,
    p_instance_id UUID DEFAULT NULL
) RETURNS BOOLEAN AS $$
DECLARE
    v_current_balance BIGINT;
    v_new_balance BIGINT;
BEGIN
    SELECT credit_balance_cents INTO v_current_balance
    FROM profiles WHERE id = p_user_id FOR UPDATE;

    IF v_current_balance < p_amount_cents THEN
        RETURN FALSE;
    END IF;

    v_new_balance := v_current_balance - p_amount_cents;

    UPDATE profiles SET credit_balance_cents = v_new_balance WHERE id = p_user_id;

    INSERT INTO credit_transactions (user_id, amount_cents, type, description, instance_id, balance_after_cents)
    VALUES (p_user_id, -p_amount_cents, 'rental_charge', p_description, p_instance_id, v_new_balance);

    RETURN TRUE;
END;
$$ LANGUAGE plpgsql;

-- Atomic credit addition with ledger entry
CREATE OR REPLACE FUNCTION add_credits(
    p_user_id UUID,
    p_amount_cents BIGINT,
    p_type transaction_type,
    p_description TEXT,
    p_stripe_pi TEXT DEFAULT NULL
) RETURNS BIGINT AS $$
DECLARE
    v_new_balance BIGINT;
BEGIN
    UPDATE profiles SET credit_balance_cents = credit_balance_cents + p_amount_cents
    WHERE id = p_user_id
    RETURNING credit_balance_cents INTO v_new_balance;

    INSERT INTO credit_transactions (user_id, amount_cents, type, description, stripe_payment_intent_id, balance_after_cents)
    VALUES (p_user_id, p_amount_cents, p_type, p_description, p_stripe_pi, v_new_balance);

    RETURN v_new_balance;
END;
$$ LANGUAGE plpgsql;

-- Enable Supabase Realtime on key tables
ALTER PUBLICATION supabase_realtime ADD TABLE instances;
ALTER PUBLICATION supabase_realtime ADD TABLE machines;

-- Row Level Security
ALTER TABLE profiles ENABLE ROW LEVEL SECURITY;
ALTER TABLE api_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE machines ENABLE ROW LEVEL SECURITY;
ALTER TABLE instances ENABLE ROW LEVEL SECURITY;
ALTER TABLE credit_transactions ENABLE ROW LEVEL SECURITY;
ALTER TABLE host_earnings ENABLE ROW LEVEL SECURITY;
ALTER TABLE payouts ENABLE ROW LEVEL SECURITY;
ALTER TABLE host_payout_settings ENABLE ROW LEVEL SECURITY;

-- Profiles policies
CREATE POLICY profiles_select ON profiles FOR SELECT USING (true);
CREATE POLICY profiles_update ON profiles FOR UPDATE USING (auth.uid() = id);

-- API Keys policies
CREATE POLICY apikeys_all ON api_keys FOR ALL USING (auth.uid() = user_id);

-- Machines policies
CREATE POLICY machines_host ON machines FOR ALL USING (auth.uid() = host_id);
CREATE POLICY machines_public ON machines FOR SELECT USING (is_available = TRUE AND status = 'online');

-- Instances policies
CREATE POLICY instances_user ON instances FOR SELECT USING (auth.uid() = user_id);
CREATE POLICY instances_host ON instances FOR SELECT USING (auth.uid() = host_id);
CREATE POLICY instances_create ON instances FOR INSERT WITH CHECK (auth.uid() = user_id);

-- Credit transactions policies
CREATE POLICY txn_user ON credit_transactions FOR SELECT USING (auth.uid() = user_id);

-- Host earnings policies
CREATE POLICY earnings_host ON host_earnings FOR SELECT USING (auth.uid() = host_id);

-- Payouts policies
CREATE POLICY payouts_host ON payouts FOR ALL USING (auth.uid() = host_id);

-- Payout settings policies
CREATE POLICY payout_settings_host ON host_payout_settings FOR ALL USING (auth.uid() = host_id);

-- Auto-create profile on signup
CREATE OR REPLACE FUNCTION handle_new_user()
RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO profiles (id, email)
    VALUES (NEW.id, NEW.email)
    ON CONFLICT (id) DO NOTHING;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

CREATE TRIGGER on_auth_user_created
    AFTER INSERT ON auth.users
    FOR EACH ROW
    EXECUTE FUNCTION handle_new_user();
