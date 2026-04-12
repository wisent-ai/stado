'use client';

import { useEffect, useState } from 'react';
import Link from 'next/link';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { formatCents } from '@/lib/utils';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';

export default function HostDashboardPage() {
  const { getAccessToken, loading: authLoading } = useAuth();
  const [machines, setMachines] = useState<any[]>([]);
  const [earnings, setEarnings] = useState<any>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (authLoading) return;
    const load = async () => {
      const token = await getAccessToken();
      if (!token) return;
      const [m, e] = await Promise.all([
        api.listMachines(token).catch(() => []),
        api.getEarnings(token).catch(() => null),
      ]);
      setMachines(m || []);
      setEarnings(e);
      setLoading(false);
    };
    load();
  }, [authLoading]);

  if (loading) return <div className="py-12 text-center text-muted-foreground">Loading...</div>;

  const onlineMachines = machines.filter((m) => m.status === 'online');

  return (
    <div>
      <div className="mb-6 flex items-center justify-between">
        <h1 className="text-3xl font-bold">Host Dashboard</h1>
        <Link href="/host/machines/register"><Button>Register Machine</Button></Link>
      </div>

      <div className="mb-8 grid gap-6 md:grid-cols-3">
        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">This Month</CardTitle></CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">{earnings ? formatCents(earnings.this_month_cents) : '$0.00'}</p>
          </CardContent>
        </Card>
        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">Total Earned</CardTitle></CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">{earnings ? formatCents(earnings.total_earned_cents) : '$0.00'}</p>
          </CardContent>
        </Card>
        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">Machines</CardTitle></CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">{onlineMachines.length}<span className="text-lg text-muted-foreground">/{machines.length}</span></p>
            <p className="text-sm text-muted-foreground">online</p>
          </CardContent>
        </Card>
      </div>

      <h2 className="mb-4 text-xl font-semibold">Your Machines</h2>
      {machines.length === 0 ? (
        <Card>
          <CardContent className="py-12 text-center">
            <p className="text-muted-foreground">No machines registered yet.</p>
            <Link href="/host/machines/register"><Button className="mt-4">Register Your First Machine</Button></Link>
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-4 md:grid-cols-2">
          {machines.map((m) => (
            <Card key={m.id}>
              <CardContent className="flex items-center justify-between p-4">
                <div>
                  <p className="font-medium">{m.label || m.hostname || 'Unnamed Machine'}</p>
                  <p className="text-sm text-muted-foreground">
                    {m.gpu_count}x GPU | {m.ram_gb} GB RAM | ${(m.price_per_hour_cents / 100).toFixed(2)}/hr
                  </p>
                </div>
                <div className="flex items-center gap-3">
                  <Badge variant={m.status === 'online' ? 'success' : m.status === 'offline' ? 'secondary' : 'warning'}>
                    {m.status}
                  </Badge>
                  <Link href={`/host/machines/${m.id}`}><Button size="sm" variant="outline">Manage</Button></Link>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      <div className="mt-8">
        <Link href="/host/earnings"><Button variant="outline">View Earnings & Payouts</Button></Link>
      </div>
    </div>
  );
}
