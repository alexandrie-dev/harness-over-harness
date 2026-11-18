use indexmap::IndexMap;
pub fn en(mode: &str, justification: &str) -> String {
    format!("Allow this operation with {mode} permissions: {justification}")
}
pub fn zh(mode: &str, justification: &str) -> String {
    format!("允许本次操作使用 {mode} 权限：{justification}")
}
pub fn ja(mode: &str, justification: &str) -> String {
    format!("{mode} 権限でこの操作を許可します：{justification}")
}
pub fn es(mode: &str, justification: &str) -> String {
    format!("Permitir esta operación con permisos {mode}: {justification}")
}
pub fn fr(mode: &str, justification: &str) -> String {
    format!("Autoriser cette opération avec les permissions {mode} : {justification}")
}
pub fn de(mode: &str, justification: &str) -> String {
    format!("Diesen Vorgang mit {mode}-Berechtigungen zulassen: {justification}")
}
pub fn ru(mode: &str, justification: &str) -> String {
    format!("Разрешить эту операцию с правами {mode}: {justification}")
}
pub fn pt(mode: &str, justification: &str) -> String {
    format!("Permitir esta operação com permissões {mode}: {justification}")
}
pub fn escalation_display_reason(mode: &str, justification: &str) -> IndexMap<String, String> {
    [
        ("en", en(mode, justification)),
        ("zh", zh(mode, justification)),
        ("ja", ja(mode, justification)),
        ("es", es(mode, justification)),
        ("fr", fr(mode, justification)),
        ("de", de(mode, justification)),
        ("ru", ru(mode, justification)),
        ("pt", pt(mode, justification)),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}
