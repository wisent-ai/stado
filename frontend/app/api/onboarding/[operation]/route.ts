import { NextRequest, NextResponse } from 'next/server';

export const runtime = 'nodejs';
export const dynamic = 'force-dynamic';

const PRODUCT_ID = 'compute-marketplace';
const STADO_CLIENT = 'web';
const OPERATIONS: Record<string, true> = {
  'bundle.read': true,
  'experiments.assign': true,
  'events.collect': true,
  'state.read': true,
};
const MAX_BODY_BYTES = 128 * 1024;

function integrationConfig() {
  const rawBaseUrl = process.env.STADO_INTEGRATION_API_URL?.trim();
  const rawToken = process.env.COMPUTE_MARKETPLACE_STADO_INTEGRATION_TOKEN;
  const token = rawToken?.trim();
  const rawTimeout = process.env.STADO_INTEGRATION_TIMEOUT_MS?.trim();
  const timeoutMs = Number(rawTimeout);
  if (!rawBaseUrl || !token || !rawTimeout) throw new Error('onboarding_integration_unavailable');
  if (token !== rawToken || /[\u0000-\u001f\u007f]/u.test(token)) throw new Error('onboarding_integration_invalid');
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) throw new Error('onboarding_integration_invalid');

  let baseUrl: URL;
  try {
    baseUrl = new URL(rawBaseUrl);
  } catch {
    throw new Error('onboarding_integration_invalid');
  }
  const isDevelopmentLoopback = process.env.NODE_ENV === 'development'
    && ['localhost', '127.0.0.1', '::1', '[::1]'].includes(baseUrl.hostname);
  if ((baseUrl.protocol !== 'https:' && !(baseUrl.protocol === 'http:' && isDevelopmentLoopback))
    || baseUrl.username || baseUrl.password || baseUrl.search || baseUrl.hash
    || (baseUrl.pathname !== '' && baseUrl.pathname !== '/')) {
    throw new Error('onboarding_integration_invalid');
  }
  return { origin: baseUrl.origin, timeoutMs, token };
}

export async function POST(request: NextRequest, { params }: { params: { operation: string } }) {
  if (!OPERATIONS[params.operation]) {
    return NextResponse.json({ ok: false, error: { code: 'operation_not_found' } }, { status: 404 });
  }

  const contentLength = Number(request.headers.get('content-length') ?? 0);
  if (contentLength > MAX_BODY_BYTES) {
    return NextResponse.json({ ok: false, error: { code: 'request_too_large' } }, { status: 413 });
  }

  let body: Record<string, unknown>;
  try {
    const rawBody = await request.text();
    if (new TextEncoder().encode(rawBody).byteLength > MAX_BODY_BYTES) throw new Error('request_too_large');
    body = JSON.parse(rawBody) as Record<string, unknown>;
  } catch (error) {
    const code = error instanceof Error && error.message === 'request_too_large' ? 'request_too_large' : 'invalid_json';
    return NextResponse.json({ ok: false, error: { code } }, { status: code === 'request_too_large' ? 413 : 400 });
  }
  if (!body || Array.isArray(body) || body.product_id !== PRODUCT_ID) {
    return NextResponse.json({ ok: false, error: { code: 'invalid_product' } }, { status: 400 });
  }

  try {
    const { origin, timeoutMs, token } = integrationConfig();
    const endpoint = new URL(`/integration/${STADO_CLIENT}/onboarding/${PRODUCT_ID}/${params.operation}`, origin);
    const response = await fetch(endpoint, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      cache: 'no-store',
      signal: AbortSignal.timeout(timeoutMs),
    });
    const responseBody = await response.json();
    return NextResponse.json(responseBody, { status: response.status });
  } catch (error) {
    const code = error instanceof Error && error.message.startsWith('onboarding_integration_')
      ? error.message
      : 'onboarding_upstream_unavailable';
    return NextResponse.json({ ok: false, error: { code } }, { status: 503 });
  }
}
