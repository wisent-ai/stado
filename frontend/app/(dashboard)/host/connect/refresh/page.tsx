'use client';

import { useEffect } from 'react';
import { useRouter } from 'next/navigation';
import { useAuth } from '@/contexts/AuthContext';
import { api } from '@/lib/api';

export default function ConnectRefreshPage() {
  const router = useRouter();
  const { getAccessToken } = useAuth();

  useEffect(() => {
    const redirect = async () => {
      const token = await getAccessToken();
      if (!token) { router.push('/host/connect'); return; }
      const result = await api.connectOnboard(token).catch(() => null);
      if (result) {
        window.location.href = result.onboarding_url;
      } else {
        router.push('/host/connect');
      }
    };
    redirect();
  }, []);

  return (
    <div className="flex min-h-[50vh] items-center justify-center">
      <p className="text-muted-foreground">Refreshing onboarding link...</p>
    </div>
  );
}
