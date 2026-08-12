'use client';

import { useEffect } from 'react';
import { useRouter } from 'next/navigation';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';

export default function ConnectReturnPage() {
  const router = useRouter();

  useEffect(() => {
    const timer = setTimeout(() => router.push('/host/connect'), 3000);
    return () => clearTimeout(timer);
  }, [router]);

  return (
    <div className="flex min-h-[50vh] items-center justify-center">
      <Card className="max-w-md text-center">
        <CardHeader>
          <CardTitle>Setup Complete</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="mb-4 text-muted-foreground">
            Your Stripe Connect account has been configured.
            Redirecting to your account status...
          </p>
          <Button onClick={() => router.push('/host/connect')}>Go to Account Status</Button>
        </CardContent>
      </Card>
    </div>
  );
}
