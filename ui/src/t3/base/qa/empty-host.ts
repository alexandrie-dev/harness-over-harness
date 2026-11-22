// Проверка основы HOH-40 (только для отдельной сборки vite.config.ts с T3_BASE_QA=1, не для окна): T3 на хосте u3
// (ui/src/hoh-host) с пустым движком — поддельный движок тестов хоста без разговоров. Так снимается пустая оболочка.
// @ts-expect-error — модуль тестов хоста на JS без объявлений типов.
import { FakeEngine } from '../../../../tests/hoh-host/fake-engine.mjs';
import { installHohHost } from '../../../hoh-host/index.ts';

installHohHost({ engine: new FakeEngine() });
void import('../web/bootstrap.ts');
