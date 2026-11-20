# Уведомления окна HoH

Окно (`ui/`) использует чужие материалы. Их уведомления воспроизведены ниже, как того требуют лицензии.
Уведомления остальных частей продукта — в `NOTICE` в корне репозитория; шрифт Inter — `src/assets/fonts/LICENSE-Inter.txt`.

## T3 Code (MIT)

Размеры и устройство компонентов, а также файлы разметки ответов Помощника (`ui/src/t3/`, перечень — `ui/T3-MAP.md`) взяты из T3 Code (коммит 803f21f6), MIT License, Copyright (c) 2026 T3 Tools Inc. Имён и знаков T3 в окне нет. Патчи пакетов разбора Markdown `ui/patches/*.patch` (mdast-util-math, micromark-extension-llm-math, micromark-extension-gfm-table, micromark, mdast-util-to-hast) взяты из того же коммита T3 Code без изменений и подключены через `patchedDependencies`.

```
MIT License

Copyright (c) 2026 T3 Tools Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Значки поставщиков моделей

Рисунки значков OpenAI, Claude, OpenRouter и Ollama в `src/screens/shared/providerIcons.tsx` взяты из `apps/web/src/components/Icons.tsx` проекта T3 Code (MIT, полный текст лицензии выше). Рисунок не менялся; вместо вспомогательной функции классов подставлена строка.
