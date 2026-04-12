'use client';

import { useEffect, useState } from 'react';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { formatCents } from '@/lib/utils';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table';

export default function EarningsPage() {
  const { getAccessToken, loading: authLoading } = useAuth();
  const [earnings, setEarnings] = useState<any>(null);
  const [payouts, setPayouts] = useState<any[]>([]);
  const [loading, setLoading] = useState(true);
  const [requesting, setRequesting] = useState(false);

  useEffect(() => {
    if (authLoading) return;
    const load = async () => {
      const token = await getAccessToken();
      if (!token) return;
      const [e, p] = await Promise.all([
        api.getEarnings(token).catch(() => null),
        api.listPayouts(token).catch(() => []),
      ]);
      setEarnings(e);
      setPayouts(p || []);
      setLoading(false);
    };
    load();
  }, [authLoading]);

  const handleRequestPayout = async () => {
    setRequesting(true);
    const token = await getAccessToken();
    if (!token) return;
    await api.requestPayout(token).catch((err: Error) => alert(err.message));
    setRequesting(false);
    window.location.reload();
  };

  if (loading) return <div className="py-12 text-center text-muted-foreground">Loading...</div>;

  return (
    <div>
      <h1 className="mb-6 text-3xl font-bold">Earnings & Payouts</h1>

      <div className="mb-8 grid gap-6 md:grid-cols-4">
        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">Total Earned</CardTitle></CardHeader>
          <CardContent><p className="text-2xl font-bold">{earnings ? formatCents(earnings.total_earned_cents) : '$0.00'}</p></CardContent>
        </Card>
        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">This Month</CardTitle></CardHeader>
          <CardContent><p className="text-2xl font-bold">{earnings ? formatCents(earnings.this_month_cents) : '$0.00'}</p></CardContent>
        </Card>
        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">Pending Payout</CardTitle></CardHeader>
          <CardContent><p className="text-2xl font-bold">{earnings ? formatCents(earnings.pending_payout_cents) : '$0.00'}</p></CardContent>
        </Card>
        <Card>
          <CardHeader><CardTitle className="text-sm text-muted-foreground">Total Paid Out</CardTitle></CardHeader>
          <CardContent><p className="text-2xl font-bold">{earnings ? formatCents(earnings.total_paid_out_cents) : '$0.00'}</p></CardContent>
        </Card>
      </div>

      <div className="mb-8">
        <Button onClick={handleRequestPayout} disabled={requesting || !earnings || earnings.pending_payout_cents < 5000}>
          {requesting ? 'Requesting...' : 'Request Payout'}
        </Button>
        {earnings && earnings.pending_payout_cents < 5000 && (
          <p className="mt-2 text-sm text-muted-foreground">Minimum payout: $50.00</p>
        )}
      </div>

      <h2 className="mb-4 text-xl font-semibold">Payout History</h2>
      <Card>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Date</TableHead>
                <TableHead>Amount</TableHead>
                <TableHead>Method</TableHead>
                <TableHead>Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {payouts.map((p) => (
                <TableRow key={p.id}>
                  <TableCell>{new Date(p.requested_at).toLocaleDateString()}</TableCell>
                  <TableCell className="font-semibold">{formatCents(p.amount_cents)}</TableCell>
                  <TableCell>{p.payout_method}</TableCell>
                  <TableCell>
                    <Badge variant={p.status === 'completed' ? 'success' : p.status === 'failed' ? 'destructive' : 'warning'}>
                      {p.status}
                    </Badge>
                  </TableCell>
                </TableRow>
              ))}
              {payouts.length === 0 && (
                <TableRow>
                  <TableCell colSpan={4} className="py-8 text-center text-muted-foreground">No payouts yet.</TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
