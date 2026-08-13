'use client';

import { useEffect, useRef, useState } from 'react';
import {
  JourneyClient,
  LocalStorageJourneyStorage,
  type JourneyProgress,
  type JourneyScreen,
} from '@wisent-ai/onboarding-web';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import {
  MARKETPLACE_FALLBACK_BUNDLE,
  MARKETPLACE_ONBOARDING_CONTENT,
  MARKETPLACE_JOURNEY_ID,
  MARKETPLACE_PRODUCT_ID,
  MarketplaceJourneyTransport,
  marketplaceSubject,
} from '@/lib/onboarding/marketplace';

interface MarketplaceOnboardingProps {
  authLoading: boolean;
  userId: string | null;
  observedOfferId: string | null;
}

const SCREEN_DETAILS: Record<string, string> = {
  promise: 'Each row is a product offer backed by marketplace data. Compare the offered GPU, memory, location, reliability, and hourly rate before deciding whether it fits your workload.',
  control_model: 'The displayed rate is the machine offer price per hour; filtering or viewing an offer does not authorize a charge. Renting requires sign-in, an explicit machine selection, and workload details. That authorization boundary is where instance creation and billing begin.',
  first_success: 'Use the live table below to inspect a machine’s capacity, reliability, location, and hourly price. This step finishes only after the marketplace renders a real offer returned by the product API.',
};

export function MarketplaceOnboarding({ authLoading, userId, observedOfferId }: MarketplaceOnboardingProps) {
  const clientRef = useRef<JourneyClient | null>(null);
  const exposedRef = useRef('');
  const completingRef = useRef(false);
  const [progress, setProgress] = useState<JourneyProgress | null>(null);
  const [screen, setScreen] = useState<JourneyScreen | null>(null);

  useEffect(() => {
    if (authLoading) return;
    let cancelled = false;

    const start = async () => {
      const identity = await marketplaceSubject(userId);
      const client = new JourneyClient({
        productId: MARKETPLACE_PRODUCT_ID,
        journeyId: MARKETPLACE_JOURNEY_ID,
        subjectHash: identity.subjectHash,
        scopeKind: identity.scopeKind,
        transport: new MarketplaceJourneyTransport(),
        storage: new LocalStorageJourneyStorage('compute-marketplace.onboarding'),
        canonicalFallback: MARKETPLACE_FALLBACK_BUNDLE,
      });
      const started = await client.start('marketplace-entry');
      if (started.progress.status !== 'in_progress' && started.progress.status !== 'completed') {
        await client.resume('marketplace-entry');
      }
      await client.flush();
      if (cancelled) return;
      clientRef.current = client;
      setProgress(client.progress);
      setScreen(client.screen);
    };

    void start();
    return () => {
      cancelled = true;
      clientRef.current = null;
      exposedRef.current = '';
      completingRef.current = false;
    };
  }, [authLoading, userId]);

  useEffect(() => {
    const client = clientRef.current;
    if (!client || !progress || !screen || progress.status !== 'in_progress') return;
    const exposureKey = `${progress.attempt_id}:${screen.screen_id}`;
    if (exposedRef.current === exposureKey) return;
    exposedRef.current = exposureKey;
    void client.expose(progress.evidence_revision);
  }, [progress, screen]);

  useEffect(() => {
    const client = clientRef.current;
    if (!client || !progress || !screen || !observedOfferId || completingRef.current
      || progress.status !== 'in_progress' || screen.screen_id !== 'first_success') return;

    completingRef.current = true;
    const evidenceRevision = `offer:${observedOfferId}`;
    void client.complete(
      { machine_offer_observed: true },
      evidenceRevision,
      { offer_id: observedOfferId, source: 'marketplace_api' },
    ).then(() => {
      if (clientRef.current !== client) return;
      setProgress(client.progress);
      setScreen(client.screen);
    }).finally(() => {
      completingRef.current = false;
    });
  }, [observedOfferId, progress, screen]);

  if (!progress || !screen) return null;

  const title = MARKETPLACE_ONBOARDING_CONTENT[screen.title_key] ?? 'Marketplace first use';
  const body = MARKETPLACE_ONBOARDING_CONTENT[screen.body_key] ?? '';
  const journeyScreens = clientRef.current?.bundle?.definition.screens ?? MARKETPLACE_FALLBACK_BUNDLE.definition.screens;
  const screenIndex = journeyScreens.findIndex((entry) => entry.screen_id === screen.screen_id);
  const totalScreens = journeyScreens.length;
  const showOffers = () => document.getElementById('marketplace-offers')?.scrollIntoView({ behavior: 'smooth', block: 'start' });

  if (progress.status === 'completed') {
    return (
      <Card className="mb-6 border-primary/40 bg-primary/5">
        <CardContent className="flex flex-wrap items-center justify-between gap-4 pt-6">
          <div>
            <div className="mb-1 flex items-center gap-2">
              <Badge variant="success">Offer found</Badge>
              <span className="font-semibold">You have reached a live machine offer</span>
            </div>
            <p className="text-sm text-muted-foreground">Compare its capacity, reliability, and hourly price before authorizing a rental.</p>
          </div>
          <Button size="sm" variant="outline" onClick={showOffers}>Browse offers</Button>
        </CardContent>
      </Card>
    );
  }

  const advance = async () => {
    const client = clientRef.current;
    if (!client) return;
    if (screen.transitions.length === 0) {
      showOffers();
      return;
    }
    await client.advance(
      { machine_offer_observed: observedOfferId !== null },
      observedOfferId ? `offer:${observedOfferId}` : `screen:${screen.screen_id}`,
    );
    if (clientRef.current !== client) return;
    setProgress(client.progress);
    setScreen(client.screen);
    if (client.screen?.screen_id === 'first_success') showOffers();
  };

  return (
    <Card className="mb-6 border-primary/40">
      <CardHeader className="pb-3">
        <div className="mb-2 flex items-center justify-between gap-3">
          <Badge variant="secondary">First use</Badge>
          <span className="text-xs text-muted-foreground">
            Step {screenIndex >= 0 ? screenIndex + 1 : progress.completed_screen_ids.length + 1} of {totalScreens}
          </span>
        </div>
        <CardTitle className="text-xl">{title}</CardTitle>
      </CardHeader>
      <CardContent>
        <p className="text-sm text-muted-foreground">{body}</p>
        {SCREEN_DETAILS[screen.screen_id] && (
          <p className="mt-3 text-sm text-muted-foreground">{SCREEN_DETAILS[screen.screen_id]}</p>
        )}
        <div className="mt-5 flex items-center gap-3">
          <Button size="sm" onClick={advance}>
            {screen.screen_id === 'promise' ? 'How offers work' : screen.transitions.length > 0 ? 'Inspect live offers' : 'Show marketplace offers'}
          </Button>
          {screen.screen_id === 'first_success' && !observedOfferId && (
            <span className="text-xs text-muted-foreground">Waiting for a live offer from the marketplace…</span>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
