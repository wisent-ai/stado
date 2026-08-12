'use client';

import { useState } from 'react';
import { useRouter } from 'next/navigation';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

export default function RegisterMachinePage() {
  const { getAccessToken } = useAuth();
  const router = useRouter();
  const [step, setStep] = useState(1);
  const [label, setLabel] = useState('');
  const [priceCents, setPriceCents] = useState('');
  const [country, setCountry] = useState('');
  const [result, setResult] = useState<any>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const handleRegister = async (e: React.FormEvent) => {
    e.preventDefault();
    setError('');
    setLoading(true);
    const token = await getAccessToken();
    if (!token) { setError('Not authenticated'); setLoading(false); return; }

    const priceNum = Math.round(parseFloat(priceCents) * 100);
    if (isNaN(priceNum) || priceNum <= 0) { setError('Invalid price'); setLoading(false); return; }

    const res = await api.registerMachine(token, {
      label,
      price_per_hour_cents: priceNum,
      min_rental_hours: 1,
      country,
    }).catch((err: Error) => { setError(err.message); return null; });

    setLoading(false);
    if (res) {
      setResult(res);
      setStep(2);
    }
  };

  if (step === 2 && result) {
    return (
      <div className="mx-auto max-w-lg">
        <h1 className="mb-6 text-3xl font-bold">Machine Registered</h1>
        <Card>
          <CardHeader>
            <CardTitle>Install the Agent</CardTitle>
            <CardDescription>Run this on your GPU machine to connect it</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div>
              <label className="text-xs text-muted-foreground">Machine ID</label>
              <code className="mt-1 block rounded bg-muted p-2 text-sm break-all">{result.id}</code>
            </div>
            <div>
              <label className="text-xs text-muted-foreground">Agent Token (save this, shown only once)</label>
              <code className="mt-1 block rounded bg-muted p-2 text-sm break-all">{result.agent_token}</code>
            </div>
            <div>
              <label className="text-xs text-muted-foreground">Quick Install</label>
              <code className="mt-1 block rounded bg-muted p-2 text-sm">
                curl -sSL https://compute.wisent.com/install.sh | bash
              </code>
            </div>
            <div>
              <label className="text-xs text-muted-foreground">Or manual config (/etc/wisent-agent/config.yaml)</label>
              <pre className="mt-1 rounded bg-muted p-2 text-xs">
{`server_url: https://api.compute.wisent.com
machine_id: ${result.id}
agent_token: ${result.agent_token}
heartbeat_interval: 30s`}
              </pre>
            </div>
            <Button onClick={() => router.push('/host')} className="w-full">Go to Host Dashboard</Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-lg">
      <h1 className="mb-6 text-3xl font-bold">Register a Machine</h1>
      <Card>
        <CardHeader>
          <CardTitle>Machine Details</CardTitle>
          <CardDescription>The agent will auto-detect GPU specs once installed</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleRegister} className="space-y-4">
            <div>
              <label className="mb-1 block text-sm">Machine Label</label>
              <Input placeholder="e.g. My RTX 4090 Server" value={label} onChange={(e) => setLabel(e.target.value)} required />
            </div>
            <div>
              <label className="mb-1 block text-sm">Price ($/hr)</label>
              <Input type="number" step="0.01" placeholder="e.g. 0.50" value={priceCents} onChange={(e) => setPriceCents(e.target.value)} required />
            </div>
            <div>
              <label className="mb-1 block text-sm">Country (ISO code)</label>
              <Input placeholder="e.g. US, DE, JP" value={country} onChange={(e) => setCountry(e.target.value)} />
            </div>
            {error && <p className="text-sm text-destructive">{error}</p>}
            <Button type="submit" disabled={loading} className="w-full">
              {loading ? 'Registering...' : 'Register Machine'}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
