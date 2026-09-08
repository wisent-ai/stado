//! The two published reply bodies: the canned five-answer information
//! template and the sponsored-subscription escalation.

/// Python `_reply_body` — the canned 5-answer template.
pub fn reply_body(subscription: &str, region: &str, contact_email: &str) -> String {
    format!(
        "Hello,\n\nThank you for following up. Please find the requested \
         information below to proceed with the GPU quota increase on \
         subscription {subscription}.\n\nRegion to Enable: {region}\n\
         Deployment Model: ARM\nService Type: Compute VM\n\n\
         Planned VM Families and Cores per family in this region:\n\
         \x20 - Standard_NC24ads_A100_v4 (NCadsA100v4): 192 cores\n\
         \x20 - Standard_ND96asr_A100_v4 (NDasrA100v4): 192 cores\n\
         \x20 - Standard_NC40ads_H100_v5 (NCadsH100v5): 200 cores\n\
         \x20 - Standard_ND96isr_H100_v5 (NDisrH100v5): 200 cores\n\n\
         Use case: wisent-compute is our GPU job orchestrator. It \
         dispatches transient (per-job, on-demand, no Spot) workloads \
         for LLM activation extraction, fine-tuning, and steered \
         inference across multiple cloud providers (GCP + this Azure \
         subscription). We need Azure GPU capacity in {region} to give \
         the autoscaler regional headroom beyond GCP's regional \
         A100/H100 limits, so a burst of queued jobs is not bottlenecked \
         on one cloud's regional ceiling. All VMs are released as soon \
         as the job completes; we do not hold capacity.\n\nPlease \
         proceed with the increase. Happy to provide any additional \
         information.\n\nRegards,\nLukasz Bartoszcze\n{contact_email}"
    )
}

/// Python `_escalation_body` — the sponsored-subscription escalation.
pub fn escalation_body(subscription: &str, quota_id: &str, region: &str, email: &str) -> String {
    format!(
        "Hello,\n\nThe denial reason cited (insufficient payment history / \
         bank decline / outstanding balance) is structurally inapplicable \
         to this subscription:\n\nSubscription ID: {subscription}\n\
         Subscription quotaId: {quota_id}\n\nThis is a credit-funded \
         sponsored Azure subscription (quotaId begins with 'Sponsored_'). \
         It has no invoice/payment history to evaluate: usage is paid \
         from a Microsoft-granted credit balance, not from a customer \
         payment instrument. There is no outstanding balance (credits \
         are consumed in real time) and no prior bank decline (no bank \
         instrument is attached).\n\nPlease escalate this ticket to the \
         capacity team that handles sponsored / credit-funded \
         subscriptions, or to your manager. The quota increase for \
         {region} is needed for wisent-compute's GPU job orchestrator — \
         same use case as the prior message (LLM activation extraction + \
         fine-tuning, on-demand, no Spot, VMs released on job completion).\
         \n\nIf you cannot escalate, please indicate the correct team or \
         process and we will re-route directly.\n\nRegards,\n\
         Lukasz Bartoszcze\n{email}"
    )
}
