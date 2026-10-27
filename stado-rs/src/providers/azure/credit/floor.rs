//! The Azure credit floor the operator named, read from his ruling in Oko's
//! register: `capability:cloud@credits-floor:azure:<amount>:<currency>`,
//! the subject Oko keeps with his words (`oko rulings list --json`) and Tama
//! verifies against what he said. Stado keeps no number of its own for it,
//! so the gate and the operator's record cannot drift apart, and the
//! refusal names whose words set the floor.
//!
//! His rule is that Azure work runs only while its credits stay above a
//! floor he names. With no standing ruling naming one, nothing shows the
//! credits suffice, so a create or start is refused naming that and how
//! his floor is recorded; an Oko that cannot be read is refused the same
//! way, naming why. Several standing floors for Azure decide by the highest,
//! the one every one of his rulings agrees is not crossed.

use serde_json::Value;

use crate::cli::release_loop::allowed::{oko_json, Unanswered};

/// What precedes the amount and currency in a floor on Azure's billing
/// variant (the `azure` section of `stado billing show --json`).
const AZURE_FLOOR: &str = "capability:cloud@credits-floor:azure:";
/// What separates the amount from its currency.
const SEPARATOR: char = ':';

/// The floor one standing ruling of the operator names.
pub(crate) struct Floor {
    pub(crate) amount: f64,
    pub(crate) currency: String,
    pub(crate) ruling: String,
    pub(crate) said: String,
}

impl Floor {
    /// The floor as a refusal names it: the amount, the ruling and his words.
    pub(crate) fn named(&self) -> String {
        format!(
            "{} {} (the operator's ruling {}: \"{}\")",
            self.amount,
            self.currency,
            self.ruling,
            self.said.trim()
        )
    }
}

/// The floor a VM create or start is decided on: the operator's highest
/// standing Azure credit floor ([`floor`]), refused when it is not in the
/// currency Azure's machine prices are quoted in, since an hour of a machine
/// cannot be taken from it. `admit` and the published `standing` both read
/// it, so `stado billing show` never admits what a create or start refuses.
pub(crate) async fn usable(operation: &str) -> Result<Floor, String> {
    let floor = floor(operation).await?;
    if !floor.currency.eq_ignore_ascii_case(super::PRICE_CURRENCY) {
        return Err(format!(
            "{operation} is refused: the floor {} is not in {}, the currency Azure's machine prices are quoted in, so \
             an hour of a machine cannot be taken from it",
            floor.named(),
            super::PRICE_CURRENCY
        ));
    }
    Ok(floor)
}

/// The highest Azure credit floor a standing ruling of the operator names,
/// or why there is none to decide `operation` on.
async fn floor(operation: &str) -> Result<Floor, String> {
    let how = format!(
        "his rule is that Azure work runs only while its credits stay above the floor he names; record his words \
         naming it with `oko rulings record --subject {AZURE_FLOOR}<amount>:<currency>`"
    );
    let rows = match oko_json(&["rulings", "list", "--json"]).await {
        Ok(rows) => rows,
        Err(Unanswered::Absent(cause)) => {
            return Err(format!(
                "{operation} is refused: no Oko is installed ({cause}), so no ruling of the operator names an Azure \
                 credit floor; {how}"
            ))
        }
        Err(Unanswered::Unread(cause)) => {
            return Err(format!(
                "{operation} is refused: the operator's rulings could not be read for an Azure credit floor ({cause})"
            ))
        }
    };
    let Some(rows) = rows.as_array() else {
        return Err(format!(
            "{operation} is refused: `oko rulings list --json` answered no list: {rows}"
        ));
    };
    let mut floors = Vec::new();
    for row in rows
        .iter()
        .filter(|row| row["standing"].as_bool() == Some(true))
    {
        let Some(rest) = row["subject"]
            .as_str()
            .and_then(|subject| subject.strip_prefix(AZURE_FLOOR))
        else {
            continue;
        };
        let (Some(ruling), Some(said)) =
            (row["ruling"]["id"].as_str(), row["ruling"]["said"].as_str())
        else {
            return Err(format!(
                "{operation} is refused: an Azure credit floor ruling names no id or words: {row}"
            ));
        };
        let Some((amount, currency)) = rest.split_once(SEPARATOR) else {
            return Err(format!(
                "{operation} is refused: ruling {ruling} names the floor {rest:?}, which is not <amount>:<currency>"
            ));
        };
        let amount = amount.parse::<f64>().map_err(|error| {
            format!("{operation} is refused: ruling {ruling} names the floor amount {amount:?}: {error}")
        })?;
        if !amount.is_finite() || amount.is_sign_negative() {
            return Err(format!(
                "{operation} is refused: ruling {ruling} names the floor amount {amount}, which is no finite amount of credit at or above zero"
            ));
        }
        floors.push(Floor {
            amount,
            currency: currency.to_string(),
            ruling: ruling.to_string(),
            said: said.to_string(),
        });
    }
    // Floors in different currencies cannot be ordered against each other;
    // which one stands is the operator's to say, so both are named with the
    // two ways Oko's register settles it: his later words supersede the
    // other floor once Oko's supersession pass reads them, and a floor
    // subject bound to a ruling by mistake is taken off that ruling.
    if let Some(first) = floors.first() {
        let other = floors
            .iter()
            .find(|floor| !floor.currency.eq_ignore_ascii_case(&first.currency));
        if let Some(other) = other {
            return Err(format!(
                "{operation} is refused: the operator's standing Azure credit floors are in different currencies, {} \
                 and {}, which cannot be compared; the operator says which floor stands, and his later words \
                 supersede the other once `oko rulings supersede` has read them (`oko rulings show <id>` names \
                 superseded_by), or a floor bound to a ruling by mistake is taken off it with `oko rulings unbind \
                 <id> --subject {AZURE_FLOOR}<amount>:<currency>`",
                first.named(),
                other.named()
            ));
        }
    }
    floors
        .into_iter()
        .max_by(|left, right| left.amount.total_cmp(&right.amount))
        .ok_or_else(|| {
            format!(
                "{operation} is refused: no standing ruling of the operator names an Azure credit floor \
                 ({AZURE_FLOOR}<amount>:<currency> in Oko's register), so nothing shows the credits suffice; {how}"
            )
        })
}

/// The balance one read of the Azure billing section states, in the floor's
/// currency: refused when the section names another currency, since the
/// two cannot be compared.
pub(crate) fn in_currency(
    operation: &str,
    section: &Value,
    floor: &Floor,
    balance: f64,
) -> Result<f64, String> {
    let Some(currency) = section.get("currency").and_then(Value::as_str) else {
        return Err(format!(
            "{operation} is refused: the Azure billing section names no currency for its balance, so it cannot be \
             compared with the floor {}",
            floor.named()
        ));
    };
    match currency.eq_ignore_ascii_case(&floor.currency) {
        true => Ok(balance),
        false => Err(format!(
            "{operation} is refused: the Azure balance is in {currency} and the floor {} is not, so they cannot be \
             compared",
            floor.named()
        )),
    }
}
