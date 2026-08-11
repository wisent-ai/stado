import useSWR from 'swr';
import { api } from '@/lib/api';
import { useAuth } from '@/contexts/AuthContext';

export function useInstances() {
  const { getAccessToken } = useAuth();

  const { data, error, isLoading, mutate } = useSWR(
    'instances',
    async () => {
      const token = await getAccessToken();
      if (!token) return [];
      return api.listInstances(token);
    },
    { refreshInterval: 10000 }
  );

  return {
    instances: data || [],
    error,
    isLoading,
    refresh: mutate,
  };
}

export function useInstance(id: string) {
  const { getAccessToken } = useAuth();

  const { data, error, isLoading, mutate } = useSWR(
    id ? `instance-${id}` : null,
    async () => {
      const token = await getAccessToken();
      if (!token) return null;
      return api.getInstance(token, id);
    },
    { refreshInterval: 5000 }
  );

  return {
    instance: data,
    error,
    isLoading,
    refresh: mutate,
  };
}
