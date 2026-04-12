'use client';

import { useState, useEffect } from 'react';
import { api } from '@/lib/api';
import { useAuth } from '@/contexts/AuthContext';
import { formatCentsPerHour } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table';

interface Offer {
  id: string;
  label: string;
  gpu_model_id: number;
  gpu_count: number;
  gpu_ram_gb: number;
  cpu_model: string;
  cpu_cores: number;
  ram_gb: number;
  disk_gb: number;
  country: string;
  cuda_version: string;
  price_per_hour_cents: number;
  uptime_percentage: number;
  total_rentals: number;
}

export default function MarketplacePage() {
  const [offers, setOffers] = useState<Offer[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [maxPrice, setMaxPrice] = useState('');
  const [minVram, setMinVram] = useState('');
  const { getAccessToken } = useAuth();

  const fetchOffers = async () => {
    setLoading(true);
    const params = new URLSearchParams();
    if (maxPrice) params.set('max_price_cents', String(Number(maxPrice) * 100));
    if (minVram) params.set('min_vram_gb', minVram);
    const data = await api.listOffers(params.toString());
    setOffers(data.offers || []);
    setTotal(data.total || 0);
    setLoading(false);
  };

  useEffect(() => { fetchOffers(); }, []);

  const handleRent = async (offer: Offer) => {
    const token = await getAccessToken();
    if (!token) {
      window.location.href = '/login';
      return;
    }
    const image = prompt('Docker image (e.g. pytorch/pytorch:2.1.0-cuda12.1-cudnn8-devel):');
    const sshKey = prompt('Your SSH public key:');
    if (!image || !sshKey) return;

    await api.createInstance(token, {
      machine_id: offer.id,
      docker_image: image,
      ssh_public_key: sshKey,
      disk_gb: 50,
    });
    alert('Instance created! Check your instances page.');
  };

  return (
    <div>
      <h1 className="mb-6 text-3xl font-bold">GPU Marketplace</h1>

      <Card className="mb-6">
        <CardHeader><CardTitle className="text-base">Filters</CardTitle></CardHeader>
        <CardContent>
          <div className="flex flex-wrap gap-4">
            <div>
              <label className="mb-1 block text-xs text-muted-foreground">Max $/hr</label>
              <Input type="number" placeholder="e.g. 1.50" value={maxPrice} onChange={(e) => setMaxPrice(e.target.value)} className="w-32" />
            </div>
            <div>
              <label className="mb-1 block text-xs text-muted-foreground">Min VRAM (GB)</label>
              <Input type="number" placeholder="e.g. 24" value={minVram} onChange={(e) => setMinVram(e.target.value)} className="w-32" />
            </div>
            <div className="flex items-end">
              <Button onClick={fetchOffers} size="sm">Apply Filters</Button>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">{total} GPUs Available</CardTitle>
        </CardHeader>
        <CardContent>
          {loading ? (
            <p className="text-muted-foreground">Loading offers...</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>GPU</TableHead>
                  <TableHead>VRAM</TableHead>
                  <TableHead>CPU</TableHead>
                  <TableHead>RAM</TableHead>
                  <TableHead>Disk</TableHead>
                  <TableHead>Location</TableHead>
                  <TableHead>Reliability</TableHead>
                  <TableHead>Price</TableHead>
                  <TableHead></TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {offers.map((offer) => (
                  <TableRow key={offer.id}>
                    <TableCell className="font-medium">
                      {offer.label || 'GPU Machine'}
                      {offer.gpu_count > 1 && <Badge variant="secondary" className="ml-2">{offer.gpu_count}x</Badge>}
                    </TableCell>
                    <TableCell>{offer.gpu_ram_gb} GB</TableCell>
                    <TableCell>{offer.cpu_cores} cores</TableCell>
                    <TableCell>{offer.ram_gb} GB</TableCell>
                    <TableCell>{offer.disk_gb} GB</TableCell>
                    <TableCell>{offer.country || '---'}</TableCell>
                    <TableCell>
                      <Badge variant={offer.uptime_percentage >= 99 ? 'success' : 'warning'}>
                        {offer.uptime_percentage.toFixed(1)}%
                      </Badge>
                    </TableCell>
                    <TableCell className="font-semibold text-primary">
                      {formatCentsPerHour(offer.price_per_hour_cents)}
                    </TableCell>
                    <TableCell>
                      <Button size="sm" onClick={() => handleRent(offer)}>Rent</Button>
                    </TableCell>
                  </TableRow>
                ))}
                {offers.length === 0 && (
                  <TableRow>
                    <TableCell colSpan={9} className="text-center text-muted-foreground py-8">
                      No GPUs available matching your filters.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
