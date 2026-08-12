'use client';

import { useEffect, useState } from 'react';
import Link from 'next/link';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { formatCents, formatCentsPerHour } from '@/lib/utils';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table';

const statusVariant = (s: string) => {
  if (s === 'running') return 'success' as const;
  if (s === 'creating' || s === 'stopping') return 'warning' as const;
  if (s === 'error') return 'destructive' as const;
  return 'secondary' as const;
};

export default function InstancesPage() {
  const { getAccessToken, loading: authLoading } = useAuth();
  const [instances, setInstances] = useState<any[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (authLoading) return;
    const load = async () => {
      const token = await getAccessToken();
      if (!token) return;
      const data = await api.listInstances(token).catch(() => []);
      setInstances(data || []);
      setLoading(false);
    };
    load();
  }, [authLoading]);

  if (loading) return <div className="py-12 text-center text-muted-foreground">Loading...</div>;

  return (
    <div>
      <div className="mb-6 flex items-center justify-between">
        <h1 className="text-3xl font-bold">My Instances</h1>
        <Link href="/marketplace"><Button>Rent a GPU</Button></Link>
      </div>

      <Card>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Image</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Price</TableHead>
                <TableHead>Total Cost</TableHead>
                <TableHead>Created</TableHead>
                <TableHead></TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {instances.map((inst) => (
                <TableRow key={inst.id}>
                  <TableCell className="font-medium">{inst.label || inst.id.slice(0, 8)}</TableCell>
                  <TableCell className="max-w-[200px] truncate text-sm text-muted-foreground">{inst.docker_image}</TableCell>
                  <TableCell><Badge variant={statusVariant(inst.status)}>{inst.status}</Badge></TableCell>
                  <TableCell>{formatCentsPerHour(inst.price_per_hour_cents)}</TableCell>
                  <TableCell>{formatCents(inst.total_cost_cents)}</TableCell>
                  <TableCell className="text-sm text-muted-foreground">{new Date(inst.created_at).toLocaleDateString()}</TableCell>
                  <TableCell>
                    <Link href={`/instances/${inst.id}`}><Button size="sm" variant="outline">Details</Button></Link>
                  </TableCell>
                </TableRow>
              ))}
              {instances.length === 0 && (
                <TableRow>
                  <TableCell colSpan={7} className="py-12 text-center text-muted-foreground">
                    No instances yet. <Link href="/marketplace" className="text-primary hover:underline">Rent a GPU</Link> to get started.
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
