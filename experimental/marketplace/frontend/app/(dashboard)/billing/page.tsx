'use client';

import { useEffect, useState } from 'react';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { formatCents } from '@/lib/utils';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table';

const packages = [
  { id: 'starter', name: 'Starter', credits: '$10', price: '$10', bonus: '' },
  { id: 'builder', name: 'Builder', credits: '$50', price: '$45', bonus: '10% bonus' },
  { id: 'pro', name: 'Pro', credits: '$200', price: '$170', bonus: '15% bonus' },
  { id: 'enterprise', name: 'Enterprise', credits: '$1,000', price: '$800', bonus: '20% bonus' },
];

export default function BillingPage() {
  const { getAccessToken, loading: authLoading } = useAuth();
  const [tab, setTab] = useState<'stripe' | 'crypto'>('stripe');
  const [balance, setBalance] = useState(0);
  const [transactions, setTransactions] = useState<any[]>([]);
  const [depositAddr, setDepositAddr] = useState<string | null>(null);
  const [cryptoTxns, setCryptoTxns] = useState<{ deposits: any[]; withdrawals: any[] }>({ deposits: [], withdrawals: [] });
  const [withdrawAddr, setWithdrawAddr] = useState('');
  const [withdrawAmount, setWithdrawAmount] = useState('');
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

  const loadCrypto = async () => {
    const token = await getAccessToken();
    if (!token) return;
    const [addr, txns] = await Promise.all([
      api.getDepositAddress(token).catch(() => null),
      api.listCryptoTransactions(token).catch(() => ({ deposits: [], withdrawals: [] })),
    ]);
    if (addr) setDepositAddr(addr.address);
    setCryptoTxns(txns);
  };

  const handleWithdraw = async () => {
    const token = await getAccessToken();
    if (!token || !withdrawAddr || !withdrawAmount) return;
    const cents = Math.round(parseFloat(withdrawAmount) * 100);
    await api.withdrawCrypto(token, withdrawAddr, cents);
    setWithdrawAddr('');
    setWithdrawAmount('');
    loadCrypto();
    const bal = await api.getBalance(token).catch(() => ({ balance_cents: 0 }));
    setBalance(bal.balance_cents || 0);
  };

  useEffect(() => { if (tab === 'crypto' && !loading) loadCrypto(); }, [tab, loading]);

  if (loading) return <div className="py-12 text-center text-muted-foreground">Loading...</div>;

  return (
    <div>
      <h1 className="mb-6 text-3xl font-bold">Billing</h1>

      <Card className="mb-6">
        <CardHeader><CardTitle className="text-sm text-muted-foreground">Current Balance</CardTitle></CardHeader>
        <CardContent><p className="text-4xl font-bold">{formatCents(balance)}</p></CardContent>
      </Card>

      <div className="mb-6 flex gap-2">
        <Button variant={tab === 'stripe' ? 'default' : 'outline'} onClick={() => setTab('stripe')}>Card Payment</Button>
        <Button variant={tab === 'crypto' ? 'default' : 'outline'} onClick={() => setTab('crypto')}>Crypto (WST)</Button>
      </div>

      {tab === 'stripe' && (
        <>
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
                    <TableRow><TableCell colSpan={5} className="py-8 text-center text-muted-foreground">No transactions yet.</TableCell></TableRow>
                  )}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </>
      )}

      {tab === 'crypto' && (
        <>
          <div className="grid gap-6 md:grid-cols-2">
            <Card>
              <CardHeader><CardTitle className="text-base">Deposit WST Tokens</CardTitle></CardHeader>
              <CardContent>
                <p className="mb-3 text-sm text-muted-foreground">Send WST tokens to this address on Base network. Credits are added automatically after 12 confirmations.</p>
                {depositAddr ? (
                  <div>
                    <label className="text-xs text-muted-foreground">Your Deposit Address (Base)</label>
                    <code className="mt-1 block break-all rounded bg-muted p-3 text-sm font-mono">{depositAddr}</code>
                    <Button variant="outline" size="sm" className="mt-2" onClick={() => navigator.clipboard.writeText(depositAddr)}>Copy Address</Button>
                  </div>
                ) : (
                  <p className="text-sm text-muted-foreground">Loading deposit address...</p>
                )}
                <div className="mt-4 rounded border border-blue-500/30 bg-blue-500/10 p-3 text-sm">
                  <p>1 WST = $1.00 in credits</p>
                  <p className="text-muted-foreground">Chain: Base (Ethereum L2)</p>
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader><CardTitle className="text-base">Withdraw to Wallet</CardTitle></CardHeader>
              <CardContent>
                <p className="mb-3 text-sm text-muted-foreground">Convert credits back to WST tokens and send to your wallet.</p>
                <div className="space-y-3">
                  <div>
                    <label className="mb-1 block text-xs text-muted-foreground">Wallet Address (Base)</label>
                    <Input placeholder="0x..." value={withdrawAddr} onChange={(e) => setWithdrawAddr(e.target.value)} />
                  </div>
                  <div>
                    <label className="mb-1 block text-xs text-muted-foreground">Amount (USD)</label>
                    <Input type="number" step="0.01" placeholder="e.g. 50.00" value={withdrawAmount} onChange={(e) => setWithdrawAmount(e.target.value)} />
                  </div>
                  <Button onClick={handleWithdraw} disabled={!withdrawAddr || !withdrawAmount} className="w-full">Withdraw</Button>
                </div>
              </CardContent>
            </Card>
          </div>

          <h2 className="mb-4 mt-8 text-xl font-semibold">Crypto Transactions</h2>
          <div className="grid gap-6 md:grid-cols-2">
            <Card>
              <CardHeader><CardTitle className="text-sm">Deposits</CardTitle></CardHeader>
              <CardContent className="p-0">
                <Table>
                  <TableHeader><TableRow><TableHead>Date</TableHead><TableHead>Amount</TableHead><TableHead>Status</TableHead><TableHead>Tx</TableHead></TableRow></TableHeader>
                  <TableBody>
                    {(cryptoTxns.deposits || []).map((d) => (
                      <TableRow key={d.id}>
                        <TableCell className="text-sm">{new Date(d.created_at).toLocaleDateString()}</TableCell>
                        <TableCell className="text-green-400">{formatCents(d.amount_usd_cents)}</TableCell>
                        <TableCell><Badge variant={d.status === 'credited' ? 'success' : 'warning'}>{d.status}</Badge></TableCell>
                        <TableCell className="text-xs font-mono">{d.tx_hash?.slice(0, 10)}...</TableCell>
                      </TableRow>
                    ))}
                    {(cryptoTxns.deposits || []).length === 0 && (
                      <TableRow><TableCell colSpan={4} className="py-4 text-center text-muted-foreground text-sm">No deposits</TableCell></TableRow>
                    )}
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
            <Card>
              <CardHeader><CardTitle className="text-sm">Withdrawals</CardTitle></CardHeader>
              <CardContent className="p-0">
                <Table>
                  <TableHeader><TableRow><TableHead>Date</TableHead><TableHead>Amount</TableHead><TableHead>Status</TableHead><TableHead>To</TableHead></TableRow></TableHeader>
                  <TableBody>
                    {(cryptoTxns.withdrawals || []).map((w) => (
                      <TableRow key={w.id}>
                        <TableCell className="text-sm">{new Date(w.created_at).toLocaleDateString()}</TableCell>
                        <TableCell className="text-red-400">-{formatCents(w.amount_usd_cents)}</TableCell>
                        <TableCell><Badge variant={w.status === 'confirmed' ? 'success' : 'warning'}>{w.status}</Badge></TableCell>
                        <TableCell className="text-xs font-mono">{w.to_address?.slice(0, 10)}...</TableCell>
                      </TableRow>
                    ))}
                    {(cryptoTxns.withdrawals || []).length === 0 && (
                      <TableRow><TableCell colSpan={4} className="py-4 text-center text-muted-foreground text-sm">No withdrawals</TableCell></TableRow>
                    )}
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
          </div>
        </>
      )}
    </div>
  );
}
