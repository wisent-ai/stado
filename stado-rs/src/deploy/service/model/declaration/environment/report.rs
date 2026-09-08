use crate::deploy::service::*;

impl UnreachableProductEnvironment {
    /// Stable machine-readable category, in `registry doctor`'s vocabulary.
    pub fn kind(&self) -> &'static str {
        match self.gap {
            EnvironmentGap::UnrecordedDeclaration { .. } => "unrecorded-service-environment",
            EnvironmentGap::HostNamedByNoTarget { .. } => "untargeted-product-host",
            EnvironmentGap::PinnedServiceEnvironment { observed: Some(_) } => {
                "service-environment-drift"
            }
            EnvironmentGap::PinnedServiceEnvironment { observed: None } => {
                "unread-service-environment"
            }
        }
    }

    /// Names the product declares, for the half of the sentence every
    /// variant shares.
    fn declared_names(&self) -> Vec<&str> {
        self.declared
            .iter()
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Declared `name=value` pairs, which is the spelling that tells an
    /// operator what the unit was supposed to hold.
    fn declared_pairs(&self) -> String {
        self.declared
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<String>>()
            .join(", ")
    }

    pub fn sentence(&self) -> String {
        match &self.gap {
            EnvironmentGap::UnrecordedDeclaration {
                adopted_at,
                observed,
            } => {
                let mut sentence = format!(
                    "registry adopted {} on {} as inventory{}, recording no program, no \
                     arguments and no environment, so the document states nothing about what \
                     that unit starts with — while release_control.products.{}.environment \
                     declares {}",
                    self.unit,
                    self.host,
                    if adopted_at.is_empty() {
                        String::new()
                    } else {
                        format!(" on {adopted_at}")
                    },
                    self.product,
                    self.declared_pairs(),
                );
                match observed {
                    Some(unit) => {
                        let missing: Vec<&str> = self
                            .declared_names()
                            .into_iter()
                            .filter(|name| !unit.carries.iter().any(|held| held == name))
                            .collect();
                        sentence.push_str(&format!(
                            ". The unit file {} on this host carries {}",
                            self.path,
                            if unit.carries.is_empty() {
                                "no environment variables at all".to_string()
                            } else {
                                unit.carries.join(", ")
                            },
                        ));
                        if !unit.program.is_empty() {
                            sentence.push_str(&format!(", and runs {}", unit.program));
                        }
                        if missing.is_empty() {
                            sentence.push_str(
                                ". Every declared variable is present, so only the record is \
                                 missing: nothing in the document can confirm that, and the \
                                 next hand-edit of the unit will go unreported",
                            );
                        } else {
                            sentence.push_str(&format!(
                                ". {} declared and the unit does not carry {}, so whatever \
                                 reads {} falls back to its own default with nothing recording \
                                 that it did",
                                if missing.len() == 1 {
                                    format!("{} is", missing[0])
                                } else {
                                    format!("{} are", missing.join(" and "))
                                },
                                if missing.len() == 1 { "it" } else { "them" },
                                if missing.len() == 1 { "it" } else { "them" },
                            ));
                        }
                    }
                    None => sentence.push_str(&format!(
                        ". The unit file {} was not read: {} is not the host this ran on, and \
                         `registry doctor` answers from the store and never sshes. Read what \
                         it carries with `stado service env {} {}`",
                        self.path, self.host, self.host, self.unit
                    )),
                }
                sentence.push_str(&format!(
                    ". Record what the unit runs and carries with `stado service adopt {} {}` \
                     so a later read has something to disagree with",
                    self.host, self.unit
                ));
                sentence
            }
            EnvironmentGap::PinnedServiceEnvironment { observed } => match observed {
                Some(unit) => {
                    let differences = self
                        .declared
                        .iter()
                        .filter_map(|(name, expected)| {
                            let actual = unit.env.get(name);
                            (actual != Some(expected)).then(|| {
                                format!(
                                    "{name}: expected {expected}, observed {}",
                                    actual.map(String::as_str).unwrap_or("<absent>")
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!(
                        "{} pins {} in the service record for {}, but its native unit {} \
                         differs: {}. Reconcile that service with `stado service ensure {} \
                         --host {}`",
                        self.host,
                        self.declared_pairs(),
                        self.unit,
                        self.path,
                        differences,
                        self.unit,
                        self.host
                    )
                }
                None => format!(
                    "{} pins {} in the service record for {}, so a delivery path exists, \
                     but the native unit {} could not be read here. Its environment is \
                     unverified; run `stado registry doctor` on {}",
                    self.host,
                    self.declared_pairs(),
                    self.unit,
                    self.path,
                    self.host
                ),
            },
            EnvironmentGap::HostNamedByNoTarget { named_hosts } => {
                let mut sentence = format!(
                    "{} declares managed_versions.{} and runs {}, but this product's \
                     release_control target map does not name {}",
                    self.host, self.product, self.unit, self.host,
                );
                if named_hosts.is_empty() {
                    sentence.push_str(&format!(
                        "; release_control.products.{}.targets is empty",
                        self.product
                    ));
                } else {
                    sentence.push_str(&format!(
                        "; release_control.products.{}.targets names {} instead",
                        self.product,
                        named_hosts.join(" and ")
                    ));
                }
                sentence.push_str(&format!(
                    ". The release agent applies products.{}.environment only for a host that \
                     map names, so {} cannot reach {} by any delivery path, and every product \
                     check here resolves the same targets entry first and so skips this host \
                     rather than reporting it. Either name {} in \
                     release_control.products.{}.targets, or pin {} in {} on this host",
                    self.product,
                    self.declared_pairs(),
                    self.host,
                    self.host,
                    self.product,
                    self.declared_names().join(" and "),
                    self.path,
                ));
                sentence
            }
        }
    }

    pub fn to_json(&self) -> Value {
        let declared: Map<String, Value> = self
            .declared
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        let gap = match &self.gap {
            EnvironmentGap::UnrecordedDeclaration {
                adopted_at,
                observed,
            } => json!({
                "kind": "unrecorded-declaration",
                "adopted_at": adopted_at,
                "unit_carries": observed.as_ref().map(|unit| unit.carries.clone()),
                "unit_program": observed.as_ref().map(|unit| unit.program.clone()),
            }),
            EnvironmentGap::HostNamedByNoTarget { named_hosts } => json!({
                "kind": "host-named-by-no-target",
                "named_hosts": named_hosts,
            }),
            EnvironmentGap::PinnedServiceEnvironment { observed } => json!({
                "kind": "pinned-service-environment",
                "observed": observed.as_ref().map(|unit| {
                    self.declared.iter().map(|(name, _)| {
                        (name.clone(), json!(unit.env.get(name)))
                    }).collect::<Map<String, Value>>()
                }),
            }),
        };
        json!({
            "host": self.host,
            "product": self.product,
            "declared": declared,
            "unit": self.unit,
            "path": self.path,
            "gap": gap,
        })
    }
}

/// True when PRODUCT is named as a whole delimited run of UNIT's identifier.
///
/// Launchd labels are dot- and dash-delimited
/// (`com.wisent.compute.service.skarbiec-control-plane`), so the product a
/// unit serves is a delimited segment of its label rather than a substring
/// of it. Both sides are canonicalised to one delimiter and fenced with it,
/// which is what lets a product whose own name carries a delimiter match:
/// half the products in this fleet are spelled `weles-worker` or
/// `image-video-router`, and comparing single tokens would have matched
/// none of them — a check that silently matches nothing is the defect this
/// one was written to catch.
///
/// Fenced and not `contains`: a bare `contains` reads `skarbiec` out of
/// `com.wisent.skarbiecd`, and a check that fires on a coincidence is a
/// check an operator turns off.
pub(super) fn unit_names_product(unit: &str, product: &str) -> bool {
    if product.is_empty() {
        return false;
    }
    fn fenced(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('.');
        for character in text.chars() {
            out.push(match character {
                '.' | '-' | '_' | '/' => '.',
                other => other.to_ascii_lowercase(),
            });
        }
        out.push('.');
        out
    }
    fenced(unit).contains(&fenced(product))
}
