import {
  type JourneyAssignment,
  type JourneyAssignmentInput,
  type JourneyBundle,
  type JourneyDefinition,
  type JourneyRuntimeEvent,
  type JourneyTransport,
  validateJourneyBundle,
} from '@wisent-ai/onboarding-web';

export const MARKETPLACE_PRODUCT_ID = 'compute-marketplace';
export const MARKETPLACE_JOURNEY_ID = 'first-use';
export const MARKETPLACE_JOURNEY_VERSION = '2026-08-04.1';

export const MARKETPLACE_ONBOARDING_CONTENT: Record<string, string> = {
  'compute-marketplace.onboarding.promise.title': 'Find authorized compute without losing control',
  'compute-marketplace.onboarding.promise.body': 'The Stado Marketplace shows machines offered under explicit price and authorization terms.',
  'compute-marketplace.onboarding.control_model.title': 'An offer is not a running workload',
  'compute-marketplace.onboarding.control_model.body': 'Review capacity, price, and authorization before selecting compute.',
  'compute-marketplace.onboarding.first_success.title': 'Inspect a real machine offer',
  'compute-marketplace.onboarding.first_success.body': 'The journey completes when a live marketplace offer is visible.',
};

const CANONICAL_DEFINITION = '{"analytics_contract":{"completion_event":"onboarding_completed","contract_version":"1","exposure_event":"onboarding_step_viewed","first_success_event":"onboarding_first_success_observed","primary_action_event":"onboarding_step_completed","surface":"web_first_use"},"entry_screen_id":"promise","first_success_fact":"machine_offer_observed","journey_id":"first-use","journey_version":"2026-08-04.1","product_id":"compute-marketplace","published_at":"2026-08-04T00:00:00Z","schema_version":1,"screens":[{"actions":["continue"],"body_key":"compute-marketplace.onboarding.promise.body","presentation":{"body":"The Stado Marketplace shows machines offered under explicit price and authorization terms.","renderer":"promise","title":"Find authorized compute without losing control"},"required":true,"screen_id":"promise","screen_kind":"promise","title_key":"compute-marketplace.onboarding.promise.title","transitions":[{"next_screen_id":"control_model","priority":10,"reason_code":"canonical_progression"}]},{"actions":["continue"],"body_key":"compute-marketplace.onboarding.control_model.body","presentation":{"body":"Review capacity, price, and authorization before selecting compute.","renderer":"explanation","title":"An offer is not a running workload"},"required":true,"screen_id":"control_model","screen_kind":"explanation","title_key":"compute-marketplace.onboarding.control_model.title","transitions":[{"next_screen_id":"first_success","priority":10,"reason_code":"canonical_progression"}]},{"actions":["complete"],"body_key":"compute-marketplace.onboarding.first_success.body","completion_evidence":{"fact":"machine_offer_observed","kind":"fact","operator":"eq","value":true},"presentation":{"body":"The journey completes when a live marketplace offer is visible.","renderer":"first_success","title":"Inspect a real machine offer"},"required":true,"screen_id":"first_success","screen_kind":"first_success","title_key":"compute-marketplace.onboarding.first_success.title","transitions":[]}],"source_revision":"compute-marketplace-first-use-2026-08-04"}';

export const MARKETPLACE_FALLBACK_BUNDLE: JourneyBundle = {
  journey_version_id: '10000000-0000-4000-8000-000000000011',
  definition: JSON.parse(CANONICAL_DEFINITION),
  canonical_definition: CANONICAL_DEFINITION,
  content_sha256: '04ba2aabbf921fdcc80ce6812ab53b10dd70a980cf34b3d1cfde2fe8946668f6',
  source_revision: 'compute-marketplace-first-use-2026-08-04',
};

interface StadoEnvelope<T> {
  ok: boolean;
  result?: T;
  error?: { code?: string };
}
type JourneyScreenWithNullableOptions = Omit<
  JourneyDefinition['screens'][number],
  'completion_evidence' | 'entry_conditions' | 'fallback_screen_id'
> & {
  completion_evidence?: JourneyDefinition['screens'][number]['completion_evidence'] | null;
  entry_conditions?: JourneyDefinition['screens'][number]['entry_conditions'] | null;
  fallback_screen_id?: string | null;
};

type JourneyDefinitionWithNullableOptions = Omit<JourneyDefinition, 'experiment_contract' | 'screens'> & {
  experiment_contract?: JourneyDefinition['experiment_contract'] | null;
  screens: JourneyScreenWithNullableOptions[];
};

async function runtimeCompatibleBundle(bundle: JourneyBundle, productId: string, journeyId: string) {
  await validateJourneyBundle(bundle, productId, journeyId);
  if (bundle.journey_version_id !== MARKETPLACE_FALLBACK_BUNDLE.journey_version_id
    || bundle.definition.journey_version !== MARKETPLACE_JOURNEY_VERSION
    || bundle.definition.first_success_fact !== 'machine_offer_observed') {
    throw new Error('marketplace journey contract is invalid');
  }
  const definition = JSON.parse(bundle.canonical_definition) as JourneyDefinitionWithNullableOptions;
  if (definition.experiment_contract === null) delete definition.experiment_contract;
  for (const screen of definition.screens) {
    if (screen.completion_evidence === null) delete screen.completion_evidence;
    if (screen.entry_conditions === null) delete screen.entry_conditions;
    if (screen.fallback_screen_id === null) delete screen.fallback_screen_id;
  }
  const [promiseScreen, controlScreen, successScreen] = definition.screens;
  const contentIsProductOwned = definition.screens.every((screen) =>
    MARKETPLACE_ONBOARDING_CONTENT[screen.title_key]
    && MARKETPLACE_ONBOARDING_CONTENT[screen.body_key]
    && screen.actions.every((action) => action === 'continue' || action === 'complete'));
  const completionEvidence = successScreen?.completion_evidence;
  if (definition.entry_screen_id !== 'promise' || definition.screens.length !== 3
    || promiseScreen?.screen_id !== 'promise'
    || promiseScreen.transitions.length !== 1
    || promiseScreen.transitions[0].next_screen_id !== 'control_model'
    || controlScreen?.screen_id !== 'control_model'
    || controlScreen.transitions.length !== 1
    || controlScreen.transitions[0].next_screen_id !== 'first_success'
    || successScreen?.screen_id !== 'first_success'
    || successScreen.transitions.length !== 0
    || !completionEvidence || completionEvidence.kind !== 'fact'
    || completionEvidence.fact !== 'machine_offer_observed'
    || completionEvidence.operator !== 'eq' || completionEvidence.value !== true
    || !contentIsProductOwned) {
    throw new Error('marketplace journey graph is invalid');
  }
  const canonicalDefinition = JSON.stringify(definition);
  return {
    ...bundle,
    definition: definition as JourneyDefinition,
    canonical_definition: canonicalDefinition,
    content_sha256: await sha256(canonicalDefinition),
  };
}

export class MarketplaceJourneyTransport implements JourneyTransport {
  #unavailable = false;

  async post<T>(operation: string, body: unknown): Promise<T> {
    if (this.#unavailable) throw new Error('Onboarding transport is offline');
    try {
      const response = await fetch(`/api/onboarding/${operation}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      const envelope = await response.json() as StadoEnvelope<T>;
      if (!response.ok || !envelope.ok || envelope.result === undefined) {
        throw new Error(`Onboarding transport failed: ${envelope.error?.code ?? response.status}`);
      }
      return envelope.result;
    } catch (error) {
      this.#unavailable = true;
      throw error;
    }
  }

  async readBundle(productId: string, journeyId: string, journeyVersion?: string) {
    const bundle = await this.post<JourneyBundle>('bundle.read', {
      product_id: productId,
      journey_id: journeyId,
      journey_version: journeyVersion ?? MARKETPLACE_JOURNEY_VERSION,
      if_none_match: null,
    });
    return runtimeCompatibleBundle(bundle, productId, journeyId);
  }

  async readState(productId: string, attemptId: string, subjectHash: string) {
    const result = await this.post<{ found?: boolean; attempt?: unknown; answers?: unknown }>('state.read', {
      product_id: productId,
      attempt_id: attemptId,
      subject_hash: subjectHash,
    });
    return result.found === false ? null : result;
  }

  async collectEvent(event: JourneyRuntimeEvent) {
    await this.post<unknown>('events.collect', event);
  }

  assignExperiment(input: JourneyAssignmentInput) {
    return this.post<JourneyAssignment>('experiments.assign', input);
  }
}

async function sha256(value: string) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

export async function marketplaceSubject(userId: string | null) {
  if (userId) {
    return { subjectHash: await sha256(`compute-marketplace:user:${userId}`), scopeKind: 'user' as const };
  }

  const key = 'compute-marketplace.onboarding.device-id';
  let deviceId = localStorage.getItem(key);
  if (!deviceId) {
    deviceId = crypto.randomUUID();
    localStorage.setItem(key, deviceId);
  }
  return { subjectHash: await sha256(`compute-marketplace:device:${deviceId}`), scopeKind: 'device' as const };
}
