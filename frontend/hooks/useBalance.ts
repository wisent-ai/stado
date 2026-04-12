import useSWR from 'swr';
import { api } from '@/lib/api';
import { useAuth } from '@/contexts/AuthContext';

export function useBalance() {
  const { getAccessToken } = useAuth();

  const { data, error, isLoading, mutate } = useSWR(
    'balance',
    async () => {
      const token = await getAccessToken();
      if (!token) return { balance_cents: 0, packages: [] };
      return api.getBalance(token);
    },
    { refreshInterval: 30000 }
  );

  return {
    balance: data?.balance_cents || 0,
    packages: data?.packages || [],
    error,
    isLoading,
    refresh: mutate,
  };
}
