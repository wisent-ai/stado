import useSWR from 'swr';
import { api } from '@/lib/api';
import { useAuth } from '@/contexts/AuthContext';

export function useMachines() {
  const { getAccessToken } = useAuth();

  const { data, error, isLoading, mutate } = useSWR(
    'host-machines',
    async () => {
      const token = await getAccessToken();
      if (!token) return [];
      return api.listMachines(token);
    },
    { refreshInterval: 15000 }
  );

  return {
    machines: data || [],
    error,
    isLoading,
    refresh: mutate,
  };
}
