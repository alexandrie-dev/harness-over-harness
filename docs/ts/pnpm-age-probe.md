# Native age gate pnpm 12.21.0

2026-11-18, `python3 scripts/tests/ui-supply/native-age.py`. Сеть и загрузка архивов отключены. Использован уже разрешённый react@19.3.1, опубликованный 2026-10-14T16:47:15.290Z; тестовый порог усилен до 100800 минут (70 дней). Это отдельный временный workspace, настройки настоящего ui не меняются.

Получен код отказа pnpm:

```text
ERR_PNPM_NO_MATURE_MATCHING_VERSION
1 version does not meet the minimumReleaseAge constraint:
react@19.3.1 was published at 2026-10-14T16:47:15.290Z,
within the minimumReleaseAge cutoff (2026-09-09T14:52:04.812Z)
```

Имя настройки и единицы подтверждены effective `pnpm -C ui config list`: minimumReleaseAge=10080 для настоящего ui. `pnpm help install` подтверждает ignore-scripts, no-optional, frozen-lockfile, offline и allow-build/allowBuilds. Политика дополнительно сверяет возраст всех 65 версий по metadata.time до получения архивов. 7-дневный отказ самой политики проверяется молодым временем публикации в case-young.json, не стареющим при повторных запусках.
