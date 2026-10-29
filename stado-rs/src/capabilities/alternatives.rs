//! What one Wisent product calls another for, and the alternatives that
//! answer it, in the order they are tried.
//!
//! A product that needs a model, a browser, a secret, a mailbox or a
//! transcript does not dial the product that provides it. It asks
//! `stado capabilities resolve FAMILY --consumer PRODUCT`, which answers the
//! first alternative in the selected order that is proven to answer, and
//! names every one before it that did not. When none answers, the answer is
//! a typed refusal naming each alternative's product, route and cause, so the
//! caller keeps doing every part of its work that does not need it. That is
//! how a product works on its own: a product that is down costs the caller
//! one named refusal, never its whole function.
//!
//! The families and their alternatives are declared here once. The selected
//! order of a family is the configuration key `capabilities.<family>.order`
//! (its environment override is the field's `env`), and the declared order
//! when that key is unset.

use super::catalog::{CapabilityKind, ProviderId};
use super::config::ConfigField;

/// How one alternative is reached, and so how it is proven to answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reach {
    /// A service the service directory places: proven by `stado service
    /// directory connect SERVICE --consumer CONSUMER`, which knocks on the
    /// route and refuses one the resolver already holds unanswered.
    Directory { service: &'static str },
    /// An OpenAI-compatible endpoint the operator declares: `url` names it,
    /// `model` the model the caller asks it for, and `key_role` the role of
    /// the credential-store item holding its key (`stado credentials get
    /// --role`), handed to the caller as that role, never as the secret. Any
    /// HTTP answer from `<url>/v1/models` proves it.
    Endpoint {
        url: &'static ConfigField,
        model: &'static ConfigField,
        key_role: &'static ConfigField,
    },
    /// A program found on this machine's PATH.
    Program { name: &'static str },
    /// The owner-only credential file `credentials.store` names as
    /// `file://PATH`.
    OwnerFile,
}

/// One alternative of a family: the provider and how it is reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Alternative {
    pub provider: ProviderId,
    pub reach: Reach,
}

/// One family: what is asked for, the key that selects its order, and its
/// alternatives in the order declared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Family {
    pub id: CapabilityKind,
    pub order: ConfigField,
    pub alternatives: &'static [Alternative],
}

pub const INFERENCE_ORDER_CONFIG: ConfigField = ConfigField::list(
    "inference-order",
    "STADO_INFERENCE_ORDER",
    "capabilities.inference.order",
);
pub const INFERENCE_ENDPOINT_URL_CONFIG: ConfigField = ConfigField::scalar(
    "inference-endpoint-url",
    "STADO_INFERENCE_ENDPOINT_URL",
    "capabilities.inference.endpoint.url",
);
pub const INFERENCE_ENDPOINT_MODEL_CONFIG: ConfigField = ConfigField::scalar(
    "inference-endpoint-model",
    "STADO_INFERENCE_ENDPOINT_MODEL",
    "capabilities.inference.endpoint.model",
);
pub const INFERENCE_ENDPOINT_KEY_ROLE_CONFIG: ConfigField = ConfigField::scalar(
    "inference-endpoint-key-role",
    "STADO_INFERENCE_ENDPOINT_KEY_ROLE",
    "capabilities.inference.endpoint.key_role",
);
pub const SECRETS_ORDER_CONFIG: ConfigField = ConfigField::list(
    "secrets-order",
    "STADO_SECRETS_ORDER",
    "capabilities.secrets.order",
);
pub const BROWSER_AUTOMATION_ORDER_CONFIG: ConfigField = ConfigField::list(
    "browser-automation-order",
    "STADO_BROWSER_AUTOMATION_ORDER",
    "capabilities.browser-automation.order",
);
pub const MAIL_ORDER_CONFIG: ConfigField =
    ConfigField::list("mail-order", "STADO_MAIL_ORDER", "capabilities.mail.order");
pub const TRANSCRIPTS_ORDER_CONFIG: ConfigField = ConfigField::list(
    "transcripts-order",
    "STADO_TRANSCRIPTS_ORDER",
    "capabilities.transcripts.order",
);

/// Every family a product resolves through Stado, each with its
/// alternatives in the declared order.
pub const FAMILIES: &[Family] = &[
    Family {
        id: CapabilityKind::Inference,
        order: INFERENCE_ORDER_CONFIG,
        alternatives: &[
            Alternative {
                provider: ProviderId::Brama,
                reach: Reach::Directory { service: "brama" },
            },
            Alternative {
                provider: ProviderId::ModelEndpoint,
                reach: Reach::Endpoint {
                    url: &INFERENCE_ENDPOINT_URL_CONFIG,
                    model: &INFERENCE_ENDPOINT_MODEL_CONFIG,
                    key_role: &INFERENCE_ENDPOINT_KEY_ROLE_CONFIG,
                },
            },
        ],
    },
    Family {
        id: CapabilityKind::Secrets,
        order: SECRETS_ORDER_CONFIG,
        alternatives: &[
            Alternative {
                provider: ProviderId::Skarbiec,
                reach: Reach::Directory {
                    service: "skarbiec",
                },
            },
            Alternative {
                provider: ProviderId::OwnerFile,
                reach: Reach::OwnerFile,
            },
        ],
    },
    Family {
        id: CapabilityKind::BrowserAutomation,
        order: BROWSER_AUTOMATION_ORDER_CONFIG,
        alternatives: &[Alternative {
            provider: ProviderId::Weles,
            reach: Reach::Program { name: "weles" },
        }],
    },
    Family {
        id: CapabilityKind::Mail,
        order: MAIL_ORDER_CONFIG,
        alternatives: &[Alternative {
            provider: ProviderId::Skrzynka,
            reach: Reach::Program { name: "skrzynka" },
        }],
    },
    Family {
        id: CapabilityKind::Transcripts,
        order: TRANSCRIPTS_ORDER_CONFIG,
        alternatives: &[Alternative {
            provider: ProviderId::TranscriptLake,
            reach: Reach::Program {
                name: "transcript-lake",
            },
        }],
    },
];

/// The family declared for `id`; `None` for a capability no product
/// resolves through Stado.
pub fn family(id: &str) -> Option<&'static Family> {
    FAMILIES.iter().find(|family| family.id.as_str() == id)
}

impl Family {
    /// The alternatives in `names` order: the selected order when one is
    /// given, refused naming a provider the family does not declare or names
    /// twice; every declared alternative in declared order when `names` is
    /// empty.
    pub fn ordered(&self, names: &[String]) -> Result<Vec<&'static Alternative>, String> {
        if names.is_empty() {
            return Ok(self.alternatives.iter().collect());
        }
        let mut chosen: Vec<&'static Alternative> = Vec::new();
        for name in names {
            let Some(alternative) = self
                .alternatives
                .iter()
                .find(|alternative| alternative.provider.matches(name.trim()))
            else {
                return Err(format!(
                    "{} has no alternative {name:?}; its alternatives are [{}]",
                    self.id,
                    self.names().join(", ")
                ));
            };
            if chosen.contains(&alternative) {
                return Err(format!("{} names {name:?} twice in its order", self.id));
            }
            chosen.push(alternative);
        }
        Ok(chosen)
    }

    /// The declared alternatives' provider names, in declared order.
    pub fn names(&self) -> Vec<&'static str> {
        self.alternatives
            .iter()
            .map(|alternative| alternative.provider.as_str())
            .collect()
    }
}
