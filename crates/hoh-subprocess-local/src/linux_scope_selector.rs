use serde::{Deserialize, Serialize};
pub type LiteralArgvOptions = Vec<String>;
pub trait LinuxScopeObservations: Send + Sync {
    fn probe(&self) -> Result<Option<LiteralArgvOptions>, hoh_error::BoxError>;
    fn manager_listener(&self) -> Option<String>;
    fn confirm_manager(&self) -> bool;
    fn launcher_identity(&self) -> Option<String>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinuxProbeTrigger {
    First,
    ScopeNotStarted,
    NewListener,
    LauncherChanged,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScopeReason {
    ProbeAvailable,
    ManagerConfirmed,
}
impl std::fmt::Display for ScopeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ProbeAvailable => "probe-available",
            Self::ManagerConfirmed => "manager-confirmed",
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FallbackReason {
    ManagerUnconfirmed,
    NoListener,
    SameListener,
    ProbeUnavailable,
}
impl std::fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ManagerUnconfirmed => "manager-unconfirmed",
            Self::NoListener => "no-listener",
            Self::SameListener => "same-listener",
            Self::ProbeUnavailable => "probe-unavailable",
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LinuxScopeDecision {
    Scope {
        reason: ScopeReason,
        #[serde(rename = "literalArgv")]
        literal_argv: LiteralArgvOptions,
        generation: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        probe: Option<LinuxProbeTrigger>,
    },
    Fallback {
        reason: FallbackReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        probe: Option<LinuxProbeTrigger>,
    },
}
enum State {
    Unprobed(LinuxProbeTrigger),
    Available {
        literal_argv: LiteralArgvOptions,
        launcher: Option<String>,
    },
    Unavailable {
        listener: Option<String>,
    },
}
pub struct LinuxScopeSelector {
    state: State,
    generation: u64,
    observe: Box<dyn LinuxScopeObservations>,
}
impl LinuxScopeSelector {
    pub fn new(observe: impl LinuxScopeObservations + 'static) -> Self {
        Self {
            state: State::Unprobed(LinuxProbeTrigger::First),
            generation: 0,
            observe: Box::new(observe),
        }
    }
    pub fn select(&mut self) -> Result<LinuxScopeDecision, hoh_error::BoxError> {
        Ok(match &self.state {
            State::Unprobed(trigger) => return self.probe(*trigger),
            State::Available {
                literal_argv,
                launcher,
            } => {
                if self.observe.launcher_identity() != *launcher {
                    return self.probe(LinuxProbeTrigger::LauncherChanged);
                }
                if self.observe.confirm_manager() {
                    LinuxScopeDecision::Scope {
                        reason: ScopeReason::ManagerConfirmed,
                        literal_argv: literal_argv.clone(),
                        generation: self.generation,
                        probe: None,
                    }
                } else {
                    LinuxScopeDecision::Fallback {
                        reason: FallbackReason::ManagerUnconfirmed,
                        probe: None,
                    }
                }
            }
            State::Unavailable { listener } => match self.observe.manager_listener() {
                None => LinuxScopeDecision::Fallback {
                    reason: FallbackReason::NoListener,
                    probe: None,
                },
                Some(current) if Some(&current) == listener.as_ref() => {
                    LinuxScopeDecision::Fallback {
                        reason: FallbackReason::SameListener,
                        probe: None,
                    }
                }
                Some(_) => return self.probe(LinuxProbeTrigger::NewListener),
            },
        })
    }
    pub fn scope_not_started(&mut self, generation: u64) {
        if generation == self.generation {
            self.state = State::Unprobed(LinuxProbeTrigger::ScopeNotStarted);
        }
    }
    fn probe(
        &mut self,
        trigger: LinuxProbeTrigger,
    ) -> Result<LinuxScopeDecision, hoh_error::BoxError> {
        self.generation += 1;
        let listener = self.observe.manager_listener();
        let launcher = self.observe.launcher_identity();
        Ok(if let Some(literal_argv) = self.observe.probe()? {
            self.state = State::Available {
                literal_argv: literal_argv.clone(),
                launcher,
            };
            LinuxScopeDecision::Scope {
                reason: ScopeReason::ProbeAvailable,
                literal_argv,
                generation: self.generation,
                probe: Some(trigger),
            }
        } else {
            self.state = State::Unavailable { listener };
            LinuxScopeDecision::Fallback {
                reason: FallbackReason::ProbeUnavailable,
                probe: Some(trigger),
            }
        })
    }
}
