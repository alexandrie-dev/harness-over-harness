use crate::ApprovalPolicy;
use serde::{Deserialize, Deserializer};

// SHIM(hoh-kernel): Schemastery defaults explicit null as well as undefined;
// replace this deserializer and the Config.policy nullable schema annotation
// when kernel configuration resolution applies property defaults to null.
pub(crate) fn default_null_policy<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ApprovalPolicy, D::Error> {
    Ok(Option::<ApprovalPolicy>::deserialize(deserializer)?.unwrap_or_default())
}
