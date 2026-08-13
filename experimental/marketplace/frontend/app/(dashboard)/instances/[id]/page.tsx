'use client';

import { useEffect, useState } from 'react';
import { useParams, useRouter } from 'next/navigation';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { formatCents, formatCentsPerHour } from '@/lib/utils';
import { Card, CardHeader, CardTitle, CardContent, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';

export default function InstanceDetailPage() {
  const { id } = useParams<{ id: string }>();
  const { getAccessToken } = useAuth();
  const router = useRouter();
  const [instance, setInstance] = useState<any>(null);
  const [loading, setLoading] = useState(true);

  const load = async () => {
    const token = await getAccessToken();
    if (!token) return;
    const data = await api.getInstance(token, id).catch(() => null);
    setInstance(data);
    setLoading(false);
  };

  useEffect(() => { load(); }, [id]);

  const handleAction = async (action: 'start' | 'stop' | 'destroy') => {
    const token = await getAccessToken();
    if (!token) return;
    if (action === 'destroy' && !confirm('Destroy this instance? This cannot be undone.')) return;
    if (action === 'start') await api.startInstance(token, id);
    else if (action === 'stop') await api.stopInstance(token, id);
    else await api.destroyInstance(token, id);
    load();
  };

  if (loading) return <div className="py-12 text-center text-muted-foreground">Loading...</div>;
  if (!instance) return <div className="py-12 text-center text-muted-foreground">Instance not found.</div>;

  const isRunning = instance.status === 'running';
  const isStopped = instance.status === 'stopped';
  const isActive = ['creating', 'running', 'stopping'].includes(instance.status);

  return (
    <div>
      <div className="mb-6 flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold">{instance.label || `Instance ${id.slice(0, 8)}`}</h1>
          <p className="text-sm text-muted-foreground">{id}</p>
        </div>
        <Badge variant={isRunning ? 'success' : isStopped ? 'secondary' : 'warning'} className="text-base px-3 py-1">
          {instance.status}
        </Badge>
      </div>

      <div className="grid gap-6 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Connection Info</CardTitle>
            <CardDescription>SSH and access details</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {instance.ssh_host && instance.ssh_port ? (
              <div>
                <label className="text-xs text-muted-foreground">SSH Command</label>
                <code className="mt-1 block rounded bg-muted p-2 text-sm">
                  ssh root@{instance.ssh_host} -p {instance.ssh_port}
                </code>
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">SSH info will appear once the instance is running.</p>
            )}
            {instance.jupyter_url && (
              <div>
                <label className="text-xs text-muted-foreground">Jupyter</label>
                <a href={instance.jupyter_url} target="_blank" rel="noopener" className="mt-1 block text-sm text-primary hover:underline">
                  {instance.jupyter_url}
                </a>
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Billing</CardTitle>
          </CardHeader>
          <CardContent className="space-y-2">
            <div className="flex justify-between"><span className="text-muted-foreground">Rate</span><span>{formatCentsPerHour(instance.price_per_hour_cents)}</span></div>
            <div className="flex justify-between"><span className="text-muted-foreground">Total Cost</span><span className="font-semibold">{formatCents(instance.total_cost_cents)}</span></div>
            <div className="flex justify-between"><span className="text-muted-foreground">Created</span><span>{new Date(instance.created_at).toLocaleString()}</span></div>
            {instance.started_at && <div className="flex justify-between"><span className="text-muted-foreground">Started</span><span>{new Date(instance.started_at).toLocaleString()}</span></div>}
          </CardContent>
        </Card>

        <Card>
          <CardHeader><CardTitle className="text-base">Configuration</CardTitle></CardHeader>
          <CardContent className="space-y-2 text-sm">
            <div className="flex justify-between"><span className="text-muted-foreground">Docker Image</span><span className="max-w-[250px] truncate">{instance.docker_image}</span></div>
            <div className="flex justify-between"><span className="text-muted-foreground">Disk</span><span>{instance.disk_gb} GB</span></div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader><CardTitle className="text-base">Actions</CardTitle></CardHeader>
          <CardContent className="flex flex-wrap gap-3">
            {isStopped && <Button onClick={() => handleAction('start')}>Start</Button>}
            {isRunning && <Button variant="secondary" onClick={() => handleAction('stop')}>Stop</Button>}
            {isActive && <Button variant="destructive" onClick={() => handleAction('destroy')}>Destroy</Button>}
            {instance.status === 'destroyed' && <p className="text-sm text-muted-foreground">This instance has been destroyed.</p>}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
