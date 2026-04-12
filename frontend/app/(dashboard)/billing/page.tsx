'use client';

import { useEffect, useState } from 'react';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { formatCents } from '@/lib/utils';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table';

const packages = [
  { id: 'starter', name: 'Starter', credits: '$10', price: '$10', bonus: '' },
  { id: 'builder', name: 'Builder', credits: '$50', price: '$45', bonus: '10% bonus' },
  { id: 'pro', name: 'Pro', credits: '$200', price: '$170', bonus: '15% bonus' },
  { id: 'enterprise', name: 'Enterprise', credits: '$1,000', price: '$800', bonus: '20% bonus' },
];

export default function BillingPage() {
  const { getAccessToken, loading: authLoading } = useAuth();
  const [balance, setBalance] = useState(0);
  const [transactions, setTransactions] = useState<any[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (authLoading) return;
    const load = async () => {
      const token = await getAccessToken();
      if (!token) return;
      const [bal, txns] = await Promise.all([
        api.getBalance(token).catch(() => ({ balance_cents: 0 })),
        api.listTransactions(token).catch(() => []),
      ]);
      setBalance(bal.balance_cents || 0);
      setTransactions(txns || []);
      setLoading(false);
    };
    load();
  }, [authLoading]);

  const handlePurchase = async (packageId: string) => {
    const token = await getAccessToken();
    if (!token) return;
    const { checkout_url } = await api.checkout(token, packageId);
    window.location.href = checkout_url;
  };

  if (loading) return <div className="py-12 text-center text-muted-foreground">Loading...</div>;

  return (
    <div>
      <h1 className="mb-6 text-3xl font-bold">Billing</h1>

      <Card className="mb-6">
        <CardHeader><CardTitle className="text-sm text-muted-foreground">Current Balance</CardTitle></CardHeader>
        <CardContent>
          <p className="text-4xl font-bold">{formatCents(balance)}</p>
        </CardContent>
      </Card>

      <h2 className="mb-4 text-xl font-semibold">Add Credits</h2>
      <div className="mb-8 grid gap-4 md:grid-cols-4">
        {packages.map((pkg) => (
          <Card key={pkg.id} className="relative">
            {pkg.bonus && <Badge className="absolute right-3 top-3" variant="success">{pkg.bonus}</Badge>}
            <CardHeader><CardTitle>{pkg.name}</CardTitle></CardHeader>
            <CardContent>
              <p className="text-2xl font-bold">{pkg.credits}</p>
              <p className="text-sm text-muted-foreground">for {pkg.price}</p>
              <Button onClick={() => handlePurchase(pkg.id)} className="mt-4 w-full" size="sm">Purchase</Button>
            </CardContent>
          </Card>
        ))}
      </div>

      <h2 className="mb-4 text-xl font-semibold">Transaction History</h2>
      <Card>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Date</TableHead>
                <TableHead>Type</TableHead>
                <TableHead>Description</TableHead>
                <TableHead>Amount</TableHead>
                <TableHead>Balance</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {transactions.map((t) => (
                <TableRow key={t.id}>
                  <TableCell className="text-sm">{new Date(t.created_at).toLocaleDateString()}</TableCell>
                  <TableCell><Badge variant="secondary">{t.type}</Badge></TableCell>
                  <TableCell className="text-sm">{t.description}</TableCell>
                  <TableCell className={t.amount_cents >= 0 ? 'text-green-400' : 'text-red-400'}>
                    {t.amount_cents >= 0 ? '+' : ''}{formatCents(t.amount_cents)}
                  </TableCell>
                  <TableCell>{formatCents(t.balance_after_cents)}</TableCell>
                </TableRow>
              ))}
              {transactions.length === 0 && (
                <TableRow>
                  <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">No transactions yet.</TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
