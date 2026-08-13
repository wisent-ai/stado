'use client';

import { useEffect, useState } from 'react';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table';

export default function APIKeysPage() {
  const { getAccessToken, loading: authLoading } = useAuth();
  const [keys, setKeys] = useState<any[]>([]);
  const [newKeyName, setNewKeyName] = useState('');
  const [newKey, setNewKey] = useState('');
  const [loading, setLoading] = useState(true);

  const loadKeys = async () => {
    const token = await getAccessToken();
    if (!token) return;
    const data = await api.listAPIKeys(token).catch(() => []);
    setKeys(data || []);
    setLoading(false);
  };

  useEffect(() => { if (!authLoading) loadKeys(); }, [authLoading]);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    const token = await getAccessToken();
    if (!token || !newKeyName) return;
    const res = await api.createAPIKey(token, newKeyName);
    setNewKey(res.key);
    setNewKeyName('');
    loadKeys();
  };

  const handleRevoke = async (id: string) => {
    if (!confirm('Revoke this API key?')) return;
    const token = await getAccessToken();
    if (!token) return;
    await api.revokeAPIKey(token, id);
    loadKeys();
  };

  if (loading) return <div className="py-12 text-center text-muted-foreground">Loading...</div>;

  return (
    <div>
      <h1 className="mb-6 text-3xl font-bold">API Keys</h1>

      <Card className="mb-6">
        <CardHeader>
          <CardTitle className="text-base">Create API Key</CardTitle>
          <CardDescription>Use API keys for programmatic access via CLI or scripts</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleCreate} className="flex gap-3">
            <Input placeholder="Key name (e.g. my-laptop)" value={newKeyName} onChange={(e) => setNewKeyName(e.target.value)} required className="max-w-xs" />
            <Button type="submit">Create Key</Button>
          </form>
          {newKey && (
            <div className="mt-4 rounded border border-yellow-500/50 bg-yellow-500/10 p-3">
              <p className="text-sm font-medium">Your new API key (copy now, shown only once):</p>
              <code className="mt-1 block break-all text-sm">{newKey}</code>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader><CardTitle className="text-base">Your Keys</CardTitle></CardHeader>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Prefix</TableHead>
                <TableHead>Created</TableHead>
                <TableHead>Last Used</TableHead>
                <TableHead></TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {keys.map((k) => (
                <TableRow key={k.id}>
                  <TableCell className="font-medium">{k.name}</TableCell>
                  <TableCell><code className="text-sm">{k.key_prefix}...</code></TableCell>
                  <TableCell className="text-sm">{new Date(k.created_at).toLocaleDateString()}</TableCell>
                  <TableCell className="text-sm">{k.last_used_at ? new Date(k.last_used_at).toLocaleDateString() : 'Never'}</TableCell>
                  <TableCell>
                    <Button size="sm" variant="destructive" onClick={() => handleRevoke(k.id)}>Revoke</Button>
                  </TableCell>
                </TableRow>
              ))}
              {keys.length === 0 && (
                <TableRow><TableCell colSpan={5} className="py-8 text-center text-muted-foreground">No API keys.</TableCell></TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
