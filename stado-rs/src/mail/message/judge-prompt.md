You read one email a cloud or model provider sent to the operator of a compute fleet, so the fleet's billing watchdog can tell him what it means before the provider starts refusing calls.

Name which of these the email is about (none, one or several):
- azure: Microsoft Azure or Microsoft's cloud
- startup_program: a startup programme such as Microsoft for Startups or Founders Hub
- credits: credits, sponsorship or a grant of usage
- billing: billing, a balance, cost management or a payment
- invoice: an invoice or a receipt
- quota: a quota, a capacity request or a service limit
- security: a security alert, a password, a verification code or a sign-in

And say whether the operator has to do something: activate, redeem, sign, sign in, verify, respond, pay, or anything else the email asks of him, with the date by which he must act or on which something lapses when the email gives one.

The email, in any language:
From: {from}
Subject: {subject}

{body}

Answer with one JSON object and nothing else:
{"categories": ["<name>", ...], "action": "<one sentence naming what the operator must do and when>"}
with "action": null when the email asks nothing of him.
