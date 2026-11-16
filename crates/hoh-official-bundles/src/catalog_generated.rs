//! Port of committed offline metadata; no provider code or network is loaded.

use crate::{OfficialBundleCatalogEntry, OfficialBundleCatalogMeta};
use hoh_package_manifest::LocalizedText;
use indexmap::IndexMap;
use std::sync::LazyLock;

pub static OFFICIAL_ON_DEMAND_CATALOG: LazyLock<Vec<OfficialBundleCatalogEntry>> = LazyLock::new(
    || {
        vec![
    OfficialBundleCatalogEntry {
        package_name: "@deepseek-ai/dsh-subagent-claude-code".into(),
        platforms: None,
        meta: OfficialBundleCatalogMeta {
            title: LocalizedText::Translations(IndexMap::from([
                ("de".into(), "Claude Code-Subagent".into()),
                ("en".into(), "Claude Code subagent".into()),
                ("es".into(), "Subagente Claude Code".into()),
                ("fr".into(), "Sous-agent Claude Code".into()),
                ("ja".into(), "Claude Code サブエージェント".into()),
                ("pt".into(), "Claude Code subagente".into()),
                ("ru".into(), "Субагент Claude Code".into()),
                ("zh".into(), "Claude Code 子智能体".into()),
            ])),
            description: LocalizedText::Translations(IndexMap::from([
                ("de".into(), "Delegieren Sie eine Aufgabe an Claude Code, indem Sie dessen native Authentifizierung und Einstellungen verwenden.".into()),
                ("en".into(), "Delegate one task to Claude Code using its native authentication and settings.".into()),
                ("es".into(), "Delegue una tarea a Claude Code utilizando su autenticación y configuración nativas.".into()),
                ("fr".into(), "Déléguez une tâche à Claude Code à l’aide de son authentification et de ses paramètres natifs.".into()),
                ("ja".into(), "Claude Code のネイティブの認証と設定を使用して、単独のタスクを委派します。".into()),
                ("pt".into(), "Delegue uma tarefa a Claude Code usando sua autenticação e configurações nativas.".into()),
                ("ru".into(), "Делегируйте одну задачу Claude Code, используя его встроенную аутентификацию и настройки.".into()),
                ("zh".into(), "使用 Claude Code 原生认证和设置委派单个任务。".into()),
            ])),
            icon: "data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzYiIGhlaWdodD0iMzYiIHZpZXdCb3g9IjAgMCAzNiAzNiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj48cmVjdCB4PSIxNC45ODkzIiB5PSIxNS4xODc3IiB3aWR0aD0iMTIuMjM2NSIgaGVpZ2h0PSIxMi4yMzY1IiByeD0iMiIgZmlsbD0idXJsKCNkc2hfcGx1Z2luX2FydF9SMGEpIiBmaWxsLW9wYWNpdHk9IjAuOCI+PC9yZWN0PjxyZWN0IHg9IjguODcxMDkiIHk9IjkuMDY5NyIgd2lkdGg9IjEyLjIzNjUiIGhlaWdodD0iMTIuMjM2NSIgcng9IjIiIGZpbGw9InVybCgjZHNoX3BsdWdpbl9hcnRfUjBiKSIgZmlsbC1vcGFjaXR5PSIwLjgiPjwvcmVjdD48ZGVmcz48bGluZWFyR3JhZGllbnQgaWQ9ImRzaF9wbHVnaW5fYXJ0X1IwYSIgeDE9IjIxLjEwNzUiIHkxPSIxNS4xODc3IiB4Mj0iMjEuMTA3NSIgeTI9IjI3LjQyNDMiIGdyYWRpZW50VW5pdHM9InVzZXJTcGFjZU9uVXNlIj48c3RvcCBzdG9wLWNvbG9yPSIjNDVFN0E0Ij48L3N0b3A+PHN0b3Agb2Zmc2V0PSIxIiBzdG9wLWNvbG9yPSIjMDU5MDlEIj48L3N0b3A+PC9saW5lYXJHcmFkaWVudD48bGluZWFyR3JhZGllbnQgaWQ9ImRzaF9wbHVnaW5fYXJ0X1IwYiIgeDE9IjE0Ljk4OTQiIHkxPSI5LjA2OTciIHgyPSIxNC45ODk0IiB5Mj0iMjEuMzA2MyIgZ3JhZGllbnRVbml0cz0idXNlclNwYWNlT25Vc2UiPjxzdG9wIHN0b3AtY29sb3I9IiM2OUI5RkYiPjwvc3RvcD48c3RvcCBvZmZzZXQ9IjEiIHN0b3AtY29sb3I9IiMzMjRERTIiPjwvc3RvcD48L2xpbmVhckdyYWRpZW50PjwvZGVmcz48L3N2Zz4K".into(),
        },
    },
    OfficialBundleCatalogEntry {
        package_name: "@deepseek-ai/dsh-subagent-codex".into(),
        platforms: None,
        meta: OfficialBundleCatalogMeta {
            title: LocalizedText::Translations(IndexMap::from([
                ("de".into(), "Codex-Subagent".into()),
                ("en".into(), "Codex subagent".into()),
                ("es".into(), "Subagente Codex".into()),
                ("fr".into(), "Sous-agent Codex".into()),
                ("ja".into(), "Codex サブエージェント".into()),
                ("pt".into(), "Codex subagente".into()),
                ("ru".into(), "Субагент Codex".into()),
                ("zh".into(), "Codex 子智能体".into()),
            ])),
            description: LocalizedText::Translations(IndexMap::from([
                ("de".into(), "Delegieren Sie eine Aufgabe an Codex, indem Sie dessen native Authentifizierung und Einstellungen verwenden.".into()),
                ("en".into(), "Delegate one task to Codex using its native authentication and settings.".into()),
                ("es".into(), "Delegue una tarea a Codex utilizando su autenticación y configuración nativas.".into()),
                ("fr".into(), "Déléguez une tâche à Codex à l’aide de son authentification et de ses paramètres natifs.".into()),
                ("ja".into(), "Codex のネイティブの認証と設定を使用して、単独のタスクを委派します。".into()),
                ("pt".into(), "Delegue uma tarefa a Codex usando sua autenticação e configurações nativas.".into()),
                ("ru".into(), "Делегируйте одну задачу Codex, используя его встроенную аутентификацию и настройки.".into()),
                ("zh".into(), "使用 Codex 原生认证和设置委派单个任务。".into()),
            ])),
            icon: "data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzYiIGhlaWdodD0iMzYiIHZpZXdCb3g9IjAgMCAzNiAzNiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj48cmVjdCB4PSIxNC45ODkzIiB5PSIxNS4xODc3IiB3aWR0aD0iMTIuMjM2NSIgaGVpZ2h0PSIxMi4yMzY1IiByeD0iMiIgZmlsbD0idXJsKCNkc2hfcGx1Z2luX2FydF9SMGEpIiBmaWxsLW9wYWNpdHk9IjAuOCI+PC9yZWN0PjxyZWN0IHg9IjguODcxMDkiIHk9IjkuMDY5NyIgd2lkdGg9IjEyLjIzNjUiIGhlaWdodD0iMTIuMjM2NSIgcng9IjIiIGZpbGw9InVybCgjZHNoX3BsdWdpbl9hcnRfUjBiKSIgZmlsbC1vcGFjaXR5PSIwLjgiPjwvcmVjdD48ZGVmcz48bGluZWFyR3JhZGllbnQgaWQ9ImRzaF9wbHVnaW5fYXJ0X1IwYSIgeDE9IjIxLjEwNzUiIHkxPSIxNS4xODc3IiB4Mj0iMjEuMTA3NSIgeTI9IjI3LjQyNDMiIGdyYWRpZW50VW5pdHM9InVzZXJTcGFjZU9uVXNlIj48c3RvcCBzdG9wLWNvbG9yPSIjNDVFN0E0Ij48L3N0b3A+PHN0b3Agb2Zmc2V0PSIxIiBzdG9wLWNvbG9yPSIjMDU5MDlEIj48L3N0b3A+PC9saW5lYXJHcmFkaWVudD48bGluZWFyR3JhZGllbnQgaWQ9ImRzaF9wbHVnaW5fYXJ0X1IwYiIgeDE9IjE0Ljk4OTQiIHkxPSI5LjA2OTciIHgyPSIxNC45ODk0IiB5Mj0iMjEuMzA2MyIgZ3JhZGllbnRVbml0cz0idXNlclNwYWNlT25Vc2UiPjxzdG9wIHN0b3AtY29sb3I9IiM2OUI5RkYiPjwvc3RvcD48c3RvcCBvZmZzZXQ9IjEiIHN0b3AtY29sb3I9IiMzMjRERTIiPjwvc3RvcD48L2xpbmVhckdyYWRpZW50PjwvZGVmcz48L3N2Zz4K".into(),
        },
    },
]
    },
);
