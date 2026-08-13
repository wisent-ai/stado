'use client';

import { useEffect, useState } from 'react';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';

export default function ConnectPage() {
  const { getAccessToken, loading: authLoading } = useAuth();
  const [status, setStatus] = useState<any>(null);
  const [loading, setLoading] = useState(true);
  const [onboarding, setOnboarding] = useState(false);

  useEffect(() => {
    if (authLoading) return;
    const load = async () => {
      const token = await getAccessToken();
      if (!token) return;
      const s = await api.connectStatus(token).catch(() => ({ status: 'not_started', charges_enabled: false, payouts_enabled: false }));
      setStatus(s);
      setLoading(false);
    };
    load();
  }, [authLoading]);

  const handleOnboard = async () => {
    setOnboarding(true);
    const token = await getAccessToken();
    if (!token) { setOnboarding(false); return; }
    const result = await api.connectOnboard(token).catch((err: Error) => {
      alert(err.message);
      return null;
    });
    setOnboarding(false);
    if (result) {
      window.location.href = result.onboarding_url;
    }
  };

  if (loading) return <div className="py-12 text-center text-muted-foreground">Loading...</div>;

  const isActive = status?.status === 'active';
  const isPending = status?.status === 'pending';
  const isNotStarted = status?.status === 'not_started' || !status;

  return (
    <div className="mx-auto max-w-lg">
      <h1 className="mb-6 text-3xl font-bold">Bank Account Setup</h1>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-3">
            Stripe Connect
            {isActive && <Badge variant="success">Active</Badge>}
            {isPending && <Badge variant="warning">Pending</Badge>}
            {isNotStarted && <Badge variant="secondary">Not Connected</Badge>}
          </CardTitle>
          <CardDescription>
            Connect your bank account to receive payouts from GPU rentals.
            Wisent uses Stripe Connect for secure, instant payments.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {isActive && (
            <div className="space-y-3">
              <div className="rounded border border-green-500/30 bg-green-500/10 p-4">
                <p className="font-medium text-green-400">Your account is fully connected.</p>
                <p className="mt-1 text-sm text-muted-foreground">You can receive payouts from your GPU rental earnings.</p>
              </div>
              <div className="grid grid-cols-2 gap-4 text-sm">
                <div>
                  <span className="text-muted-foreground">Charges</span>
                  <p className="font-medium">{status.charges_enabled ? 'Enabled' : 'Disabled'}</p>
                </div>
                <div>
                  <span className="text-muted-foreground">Payouts</span>
                  <p className="font-medium">{status.payouts_enabled ? 'Enabled' : 'Disabled'}</p>
                </div>
              </div>
              <Button variant="outline" onClick={handleOnboard} className="w-full">Update Account Details</Button>
            </div>
          )}

          {isPending && (
            <div className="space-y-3">
              <div className="rounded border border-yellow-500/30 bg-yellow-500/10 p-4">
                <p className="font-medium text-yellow-400">Account setup in progress.</p>
                <p className="mt-1 text-sm text-muted-foreground">
                  {status.details_submitted
                    ? 'Your details have been submitted. Stripe is verifying your account.'
                    : 'Please complete the onboarding process to start receiving payouts.'}
                </p>
              </div>
              <Button onClick={handleOnboard} disabled={onboarding} className="w-full">
                {onboarding ? 'Loading...' : 'Continue Setup'}
              </Button>
            </div>
          )}

          {isNotStarted && (
            <div className="space-y-4">
              <div className="space-y-2 text-sm text-muted-foreground">
                <p>To receive payouts, you need to connect a bank account through Stripe.</p>
                <p>The process takes about 5 minutes and requires:</p>
                <ul className="ml-4 list-disc space-y-1">
                  <li>Your legal name and address</li>
                  <li>Bank account or debit card details</li>
                  <li>Tax identification (SSN/EIN for US, varies by country)</li>
                </ul>
              </div>
              <div className="rounded border border-blue-500/30 bg-blue-500/10 p-3 text-sm">
                <p className="font-medium">Wisent takes a 15% platform fee.</p>
                <p className="text-muted-foreground">You receive 85% of all rental revenue from your machines.</p>
              </div>
              <Button onClick={handleOnboard} disabled={onboarding} className="w-full">
                {onboarding ? 'Setting up...' : 'Connect Bank Account'}
              </Button>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
