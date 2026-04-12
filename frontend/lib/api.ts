const API_URL = process.env.NEXT_PUBLIC_API_URL || 'http://localhost:8080';

type FetchOptions = RequestInit & { token?: string };

async function apiFetch<T>(path: string, options: FetchOptions = {}): Promise<T> {
  const { token, ...fetchOptions } = options;
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(fetchOptions.headers as Record<string, string>),
  };
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  const res = await fetch(`${API_URL}${path}`, { ...fetchOptions, headers });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`API error ${res.status}: ${text}`);
  }
  return res.json();
}

export const api = {
  // Offers
  listOffers: (params?: string) => apiFetch<any>(`/api/v1/offers${params ? `?${params}` : ''}`),
  getOffer: (id: string) => apiFetch<any>(`/api/v1/offers/${id}`),

  // User
  getMe: (token: string) => apiFetch<any>('/api/v1/users/me', { token }),
  updateMe: (token: string, data: any) =>
    apiFetch<any>('/api/v1/users/me', { token, method: 'PUT', body: JSON.stringify(data) }),

  // Instances
  listInstances: (token: string) => apiFetch<any[]>('/api/v1/instances', { token }),
  createInstance: (token: string, data: any) =>
    apiFetch<any>('/api/v1/instances', { token, method: 'POST', body: JSON.stringify(data) }),
  getInstance: (token: string, id: string) => apiFetch<any>(`/api/v1/instances/${id}`, { token }),
  startInstance: (token: string, id: string) =>
    apiFetch<any>(`/api/v1/instances/${id}/start`, { token, method: 'POST' }),
  stopInstance: (token: string, id: string) =>
    apiFetch<any>(`/api/v1/instances/${id}/stop`, { token, method: 'POST' }),
  destroyInstance: (token: string, id: string) =>
    apiFetch<any>(`/api/v1/instances/${id}`, { token, method: 'DELETE' }),

  // Billing
  getBalance: (token: string) => apiFetch<any>('/api/v1/billing/balance', { token }),
  listTransactions: (token: string) => apiFetch<any[]>('/api/v1/billing/transactions', { token }),
  checkout: (token: string, packageId: string) =>
    apiFetch<{ checkout_url: string }>('/api/v1/billing/checkout', {
      token, method: 'POST', body: JSON.stringify({ package_id: packageId }),
    }),

  // Host
  listMachines: (token: string) => apiFetch<any[]>('/api/v1/host/machines', { token }),
  registerMachine: (token: string, data: any) =>
    apiFetch<any>('/api/v1/host/machines', { token, method: 'POST', body: JSON.stringify(data) }),
  getMachine: (token: string, id: string) => apiFetch<any>(`/api/v1/host/machines/${id}`, { token }),
  deleteMachine: (token: string, id: string) =>
    apiFetch<void>(`/api/v1/host/machines/${id}`, { token, method: 'DELETE' }),
  getEarnings: (token: string) => apiFetch<any>('/api/v1/host/earnings', { token }),
  listPayouts: (token: string) => apiFetch<any[]>('/api/v1/host/payouts', { token }),
  requestPayout: (token: string) =>
    apiFetch<any>('/api/v1/host/payouts', { token, method: 'POST' }),

  // API Keys
  listAPIKeys: (token: string) => apiFetch<any[]>('/api/v1/api-keys', { token }),
  createAPIKey: (token: string, name: string) =>
    apiFetch<any>('/api/v1/api-keys', { token, method: 'POST', body: JSON.stringify({ name }) }),
  revokeAPIKey: (token: string, id: string) =>
    apiFetch<void>(`/api/v1/api-keys/${id}`, { token, method: 'DELETE' }),
};
