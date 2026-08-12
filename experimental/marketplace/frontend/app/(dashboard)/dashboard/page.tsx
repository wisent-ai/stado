'use client';

import { useEffect, useState } from 'react';
import Link from 'next/link';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { formatCents } from '@/lib/utils';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';

export default function DashboardPage() {
  const { user, getAccessToken, loading: authLoading } = useAuth();
  const [balance, setBalance] = useState<number>(0);
  const [instances, setInstances] = useState<any[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (authLoading) return;
    const load = async () => {
      const token = await getAccessToken();
      if (!token) return;
      const [balData, instData] = await Promise.all([
        api.getBalance(token).catch(() => ({ balance_cents: 0 })),
        api.listInstances(token).catch(() => []),
      ]);
      setBalance(balData.balance_cents || 0);
      setInstances(instData || []);
      setLoading(false);
    };
    load();
  }, [authLoading]);

  const activeInstances = instances.filter((i) => ['creating', 'running'].includes(i.status));

  if (authLoading || loading) {
    return <div className="py-12 text-center text-muted-foreground">Loading...</div>;
  }

  return (
    <div>
      <h1 className="mb-6 text-3xl font-bold">Dashboard</h1>
      <div className="grid gap-6 md:grid-cols-3">
        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">Credit Balance</CardTitle></CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">{formatCents(balance)}</p>
            <Link href="/billing"><Button variant="outline" size="sm" className="mt-3">Add Credits</Button></Link>
          </CardContent>
        </Card>

        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">Active Instances</CardTitle></CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">{activeInstances.length}</p>
            <Link href="/instances"><Button variant="outline" size="sm" className="mt-3">View All</Button></Link>
          </CardContent>
        </Card>

        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">Quick Actions</CardTitle></CardHeader>
          <CardContent className="flex flex-col gap-2">
            <Link href="/marketplace"><Button size="sm" className="w-full">Browse GPUs</Button></Link>
            <Link href="/host/machines/register"><Button variant="outline" size="sm" className="w-full">Host Your GPU</Button></Link>
          </CardContent>
        </Card>
      </div>

      {activeInstances.length > 0 && (
        <div className="mt-8">
          <h2 className="mb-4 text-xl font-semibold">Active Instances</h2>
          <div className="grid gap-4 md:grid-cols-2">
            {activeInstances.map((inst) => (
              <Card key={inst.id}>
                <CardContent className="flex items-center justify-between p-4">
                  <div>
                    <p className="font-medium">{inst.label || inst.docker_image}</p>
                    <p className="text-sm text-muted-foreground">{inst.id.slice(0, 8)}</p>
                  </div>
                  <div className="flex items-center gap-3">
                    <Badge variant={inst.status === 'running' ? 'success' : 'warning'}>{inst.status}</Badge>
                    <Link href={`/instances/${inst.id}`}><Button size="sm" variant="outline">Details</Button></Link>
                  </div>
                </CardContent>
              </Card>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
