# DeepSeek Harness и Cordis: что переносить в Rust

Дата исследования: 2026-11-16. Только чтение: ничего из изученных репозиториев не запускалось, не собиралось и не устанавливалось. Всё, что сказано о поведении в работе (например, о hot reload), взято из исходников, документации, тестов и проектных заметок, а не из собственного запуска.

## Главное

- Первоисточники найдены и прочитаны: репозиторий `deepseek-ai/deepseek-harness` (MIT, TypeScript, developer preview), его документация и статья arXiv:2608.25512. Ядро называется Cordis и вендорено в репозиторий с 28 локальными правками.
- Статья формализует две вещи: откатываемые эффекты (моноид пар «действие, обратное» и аккумулятор обратных на владельца) и реактивные коэффекты (таблица зависимостей, изменения которой запускают и останавливают компоненты). Поверх дано исчисление из девяти правил с теоремами о точном откате, порядке зависимостей, отсутствии взаимной блокировки и совпадении итога с загрузкой с нуля. Теория категорий в статье только фон; «функторы и монады» принадлежат пересказам.
- Гарантии условны. Обратные действия и коммутативность пишет автор плагина, рантайм их не проверяет. Внешние эффекты (сеть, письма, чужие процессы) и защита от недоверенного кода теорией не покрываются. Проверка на агентных harness в статье названа будущей работой.
- В dsh плагином является всё, включая цикл агента. Изменение состава и настроек без перезапуска работает. Замена кода на лету держится на внутренностях Node, и сопровождающие предлагают её убрать.
- Механизма «каждая задача есть плагин» в dsh нет. Есть skills, запись плагинов агентом через Plugin Manager и экспериментальные «personalized pages». От исполнения сгенерированного кода с отдельным жизненным циклом проект отказался. Встроенной памяти нет.
- Размер: 420 пакетов, около 663 тыс. строк исходников и 908 тыс. строк тестов. Ядро около 5 тыс. строк, рантайм агента без интерфейса около 180 тыс., браузерный клиент 218 тыс.
- Рекомендация для Rust. Плагины ядра: статически слинкованные crates с динамической композицией на собственном ядре по образцу Cordis (не Bevy, не динамические библиотеки). Task-плагины на машине заказчика: по умолчанию декларативный bundle без кода; при необходимости скрипт во встроенном интерпретаторе JavaScript с узким набором возможностей; интеграции и чужие harness отдельными процессами по stdio.
- Объём переписывания разобран в разделе 7: опись пакетов, цикл агента, инструменты, адаптер модели, форматы хранения, разрешения, интерфейс, тесты, минимальный состав. Выводы для дизайна в разделе 8, открытые вопросы в разделе 10.

## 0. Источники и обозначения

| Обозначение | Что это | Версия |
|---|---|---|
| `[dsh]` | https://github.com/deepseek-ai/deepseek-harness, поверхностный клон | commit `39163dd30f703a79079e2d2446545cff3c453738` (2026-11-13, синхронизация релиза dsh-0.2.2-rc.3 в master) |
| `[cordis]` | https://github.com/cordiverse/cordis (upstream ядра) | commit `c9fe179ce5ac484620a387a1697409cb007d4178` (2026-11-16), `packages/core` версии 4.0.0-rc.13 |
| `[rig]` | https://github.com/0xPlaygrounds/rig (Rust, rig-harness и rig-ecs) | commit `0748a9d53616fdd1556b6b7b8e8df8008312d877` (2026-11-16) |
| `[paper]` | arXiv:2608.25512v1, «A Programming Paradigm for Spatiotemporal Composability», https://arxiv.org/abs/2608.25512 | v1 от 2026-08-26, 92 страницы, cs.PL |

Пути вида `docs/architecture.md` без префикса относятся к `[dsh]`. Номера определений и теорем относятся к `[paper]`.

Что не удалось проверить по первоисточнику (собрано в одном месте, в тексте помечено «не проверено»):

- Официальная страница `deepseek.com/harness/en/` не открылась инструментом загрузки. Сайт документации `deepseek-harness.github.io` не открывался; его исходники лежат в `[dsh]` (`docs/`, `website/`) и читались оттуда.
- GitHub Discussions и возможные обсуждения на Hacker News и Reddit не найдены и не читались. У репозитория `open_issues_count = 0`: обратная связь принимается через Discussions (`README.md`).
- Статья «How DeepSeek Harness Uses Category Theory…» (besthub.dev) известна только по поисковой выдаче, не открывалась.
- Сообщение о выходе из sandbox (securityarsenal.com), обзоры wavect.io, hyrax.dev, implicator.ai известны только по поисковой выдаче.
- Поддержка ACP отдельными агентами (Claude Code, Codex, OpenCode, Qwen Code) известна по вторичным источникам.
- Число звёзд и подобные цифры взяты из GitHub API в момент запроса; запрос по `cordiverse/cordis` и `rodrimati1992/abi_stable_crates` не вернул данных.

## 1. Репозиторий, документация, статья и что именно формализовано

### 1.1. Что где лежит

- Репозиторий: `deepseek-ai/deepseek-harness`, лицензия MIT (`LICENSE`), TypeScript, Node.js ≥ 24.9.0 (`package.json`, поле `engines`). Создан 2026-08-13; по GitHub API на 2026-11-16: 282 158 звёзд, 34 931 форк.
- `README.md` прямо называет архитектуру «everything-is-a-plugin», ядро Cordis и статью: «powered by Cordis, whose design is described in *A Programming Paradigm for Spatiotemporal Composability*» со ссылкой на arXiv:2608.25512. Там же статус: «developer preview… THERE WILL BE COMPATIBILITY-BREAKING CHANGES».
- Документация: `https://deepseek-harness.github.io/deepseek-harness/` (ссылка из `README.md`); исходники в `docs/`. Главные входы: `docs/architecture.md`, `docs/cordis-primer.md`, `docs/cordis-tutorial/01…07`, `docs/capability-seams.md`, `docs/cookbook/*`, сгенерированные каталоги `docs/tool-catalog.md`, `docs/config-catalog.md`, `docs/persistence-catalog.md`.
- Cordis в dsh не зависимость, а вендоренная копия: `vendor/cordis` (upstream 4.0.0-rc.12), `vendor/loader`, `vendor/include`, `vendor/group`, `vendor/hmr`, `vendor/timer`, `vendor/logger-console`, плюс `cosmokit` и `schemastery`. Пакеты переименованы в `@deepseek-ai/*`. Журнал локальных отличий от upstream ведётся в `vendor/README.md` (записи V1–V28): в их числе закрытие трёх дыр реентерабельного освобождения в `fiber.ts` (V6).
- Статья: авторы Yifan Shi (Peking University и DeepSeek-AI), Wei Zhang (Peking University), Tianyi Cui (DeepSeek-AI). Исходники статьи: `github.com/cordiverse/paper` (ссылка из `[cordis] README.md`). Рецензирования, площадки публикации и машинно проверенных доказательств в статье не заявлено.

### 1.2. Что формализует статья

Статья выделяет два измерения динамической композиции (§1.1):

- **temporal composability**: при удалении компонента все его изменения общего окружения полностью откатываются;
- **spatial composability**: компоненты объявляют зависимости друг от друга, а система реактивно их разрешает, предоставляет и отзывает.

Для первого вводятся **revertible effects** (в статье именно «revertible», слово «reversible» встречается в пересказах), для второго **reactive coeffects**. Оба сведены в один тип контекста («context paradigm»), а затем дано операционное исчисление с метатеорией.

#### Алгебра эффектов (§3.1)

| Объект | Определение | Смысл |
|---|---|---|
| Контекст `Γ` | тип состояния окружения | всё, что компонент может изменить |
| Эффект | элемент моноида `(Γ → Γ, ∘, id)` | изменение контекста |
| Twisted composition monoid `𝔗Γ` | пары `(f, g)`, `(f₁,g₁)∘(f₂,g₂) = (f₁∘f₂, g₂∘g₁)`, единица `(id, id)` (Def. 1) | прямое действие и кандидат в обратное; обратные накапливаются в обратном порядке |
| Effect context `∂Γ` | `Γ × (Γ → Γ)`, пара `(γ, φ)` (Def. 2) | текущее состояние и аккумулятор `φ`, который вернёт контекст в начало |
| `track(f,g)` | `(γ, φ) ↦ (f(γ), φ∘g)` (Def. 3) | выполнить и запомнить обратное |
| `recover` | `(γ, φ) ↦ (φ(γ), id)` (Def. 6) | применить аккумулятор |
| Effect function `𝔈Γ` | `Γ → Γ × (Γ → Γ)` (Def. 8) | эффект сам возвращает своё обратное в точке применения |
| Witnessed `𝔈Γ*` | `𝔈Γ` плюс доказательство `g(δ) = γ` для `(δ, g) = e(γ)` | обратное обязано откатывать только в том состоянии, где эффект применён (левый обратный, поточечно) |
| Композиция `⋄` | `(f ⋄ g)(γ) = let (δ,s)=g(γ); (ε,t)=f(δ) in (ε, s∘t)` (Def. 9) | последовательное применение с накоплением обратных |
| `effect` | `𝔈Γ → 𝔈∂Γ` (Def. 12) | подъём на уровень выше: откат эффекта сам является отслеживаемым эффектом |
| Effect iterator `ℑΓ` | `μℑ. Γ → Γ × (Γ → Γ) × Maybe(ℑ)` (Def. 17) | последовательность эффектов с границами между шагами; соответствует генератору с `yield` |

Законы:

- Теорема 4: отслеживание не меняет прямого поведения, `pr₁ ∘ track(f,g) = f ∘ pr₁`.
- Теорема 5: `track` есть гомоморфизм моноидов `𝔗Γ → (∂Γ → ∂Γ)`.
- Теорема 7: если `g(f(γ)) = γ`, то `recover(track(f,g)(γ,φ)) = recover(γ,φ)`. Инвариант корректности: `φ(γ) = γ₀`.
- Теоремы 10–11: `(𝔈Γ, ⋄)` моноид с единицей `γ ↦ (γ, id)`; `𝔈Γ*` его подмоноид.
- Теорема 13: `effect(f) ⋄ effect(g) = effect(f ⋄ g)`.
- Теорема 15: откат поднятого эффекта точно восстанавливает состояние; аккумулятор восстанавливается тогда и только тогда, когда `g ∘ f = id`; инвариант сохраняется всегда.
- Теорема 16: откат в порядке, обратном применению (LIFO), корректен без дополнительных условий.

#### Алгебра коэффектов (§3.2)

| Объект | Определение | Смысл |
|---|---|---|
| Coeffect context `Σ` | конечная частичная зависимая функция `(k : K) ⇀ 𝒱ₖ` (Def. 19) | таблица «ключ → значение», IoC-контейнер |
| `get(k)`, `set(k,v)` | `set` имеет тип `𝔈Σ*`: возвращает `(σ[k↦v], σ' ↦ σ' ∖ k)`; предусловие `k ∉ dom(σ)` (Def. 20) | предоставление зависимости само является откатываемым эффектом |
| Спецификация `𝔇Σ` | `Set(K)` (Def. 21) | что компонент требует |
| Выполнимость | `σ ⊧ d ≔ ∀k∈d. k ∈ dom(σ)` | все зависимости на месте |
| `notify_d(σ, σ')` | activating, deactivating или neutral (Def. 22) | классификация каждого изменения контекста относительно спецификации |
| Isolation `Σiso` | `(K ⇀ R) × ((r:R) ⇀ 𝒱ᵣ)`, операция `isolate(k, r)` (Def. 24–25) | один ключ разрешается в разные значения в разных контекстах (realm) |
| Interception `Σinter` | метаданные `ℳₖ` с моноидом `(ℳₖ, ⊕ₖ, εₖ)`; `get` применяет провайдера к `μ ⊕ ι(k)` (Def. 26–27) | сквозная политика доступа к зависимости; слияние правосмещённое, внешний контекст может переопределить заявку компонента |

`isolate` и `intercept` реализуются не изменением общей таблицы, а порождением производного контекста («derived realization», Def. 23), поэтому обратного действия не требуют: производный контекст просто отбрасывается.

#### Единый контекст и независимость (§3.3–3.4)

- `Γ∞ ≔ μΓ. Γ × (Γ → Γ) × Σ` (Def. 28): рекурсивный тип, объединяющий состояние, аккумулятор и таблицу зависимостей. Рекурсия даёт дерево контекстов: родитель владеет эффектами детей.
- Коэффект при ключе есть пара `(𝒱ₖ, 𝒜ₖ)`: тип значения и набор операций над ним, каждая с обратным (Def. 29). Эффект компонента обязан состоять только из операций над объявленными ключами и из предоставлений (Def. 30). Это и есть формальное содержание требования «всё через контекст».
- Равенства читаются с точностью до **наблюдательной эквивалентности** `≃`, порождённой операциями ключа (Def. 31–33): состояние не обязано восстановиться побитно (раскладка кучи, выданные идентификаторы).
- **Независимость** двух эффектов (Def. 42): каждое преобразование одного (прямое и обратное) коммутирует с каждым преобразованием другого. Теорема 43: попарно независимые эффекты можно откатывать в любом порядке. Теорема 45: операции на разных ключах независимы. Ключ называется коммутативным, если любые две его операции независимы (Def. 44); это обязательство провайдера ключа (Def. 46). Пример коммутативного ключа: таблица, где каждая регистрация получает собственный идентификатор (слушатели событий, маршруты). Пример некоммутативного: упорядоченная цепочка middleware.

#### Исчисление (§4)

- **Component** `ℭΓ = (d, p, e)`: спецификация требуемого, множество предоставляемых ключей, эффект-итератор (Def. 48).
- **Fiber** `⟨d, p, e, π, σ, τ, θ⟩`: экземпляр компонента с родителем `π`, собственной таблицей `σ`, флагом вывода из обращения `τ` и состоянием жизненного цикла `θ ∈ Inactive | Reloading(i, g, ω) | Active(g, ω) | Unloading(g, ω)`. `ω` называется committed view: отображение объявленных ключей в имена fiber-провайдеров, зафиксированное в момент активации (Def. 49).
- **Registry** `F : 𝔑 ⇀ 𝔉`. Общая таблица зависимостей не хранится, а выводится как объединение таблиц только активных fiber. У ключа один возможный провайдер (Def. 50).
- Девять правил. Оркестрация: `O-Insert`, `O-Retire`, `O-Remove`. Жизненный цикл: `L-Begin`, `L-Iter`, `L-Finish`, `L-Divert`, `L-Leave`, `L-Unload`. Правило `L-Unload` охраняется условием `¬relied`: провайдер применяет аккумулятор только после того, как ушли все, кто на него опирался (Def. 54).
- Движущая сила: сравнение committed view с target view (на кого fiber должен опираться сейчас, Def. 53). Любое расхождение запускает переход.

Метатеория (§4.3):

| Результат | Утверждение | Условия |
|---|---|---|
| Preservation (Th. 64) | правила сохраняют корректность реестра | нет |
| Recovery exactness (Th. 68), Terminal recovery (Cor. 69) | применение аккумулятора fiber снимает ровно его вклад, что бы ни делали другие fiber между его шагами | эффекты честно свидетельствованы; с точностью до `≃` |
| Ordering (Th. 70) | fiber стартует только при выполненных зависимостях; провайдер отзывает привязку только после деактивации всех потребителей; потребитель читает зависимость в течение всего своего teardown | нет |
| Resolution coherence (Th. 71) | один переход не выполняется против двух разных разрешений зависимостей | нет |
| Progress (Th. 73) | нет взаимной блокировки, система приходит в покой | граф «предоставляет → требует» ацикличен, итераторы ограничены, множество имён конечно |
| Confluence (Th. 80) | состояние покоя определяется только итоговой конфигурацией и совпадает с загрузкой этой конфигурации с нуля | каждый компонент устанавливает все объявленные ключи; упавшие fiber исключены |

Расширения (§4.4): асинхронность (переход «инертен»: начатая итерация доходит до конца, потом fiber сворачивается), отказ (ошибка в итерации откатывает накопленное, fiber помечается failed и не перезапускается сам), изоляция (ключи как пары `K × R`), конфигурация (ревизия есть «вывести и вставить заново под тем же именем»).

### 1.3. Какие практические гарантии следуют

| Гарантия | Откуда | Оговорка самой статьи |
|---|---|---|
| Чистая выгрузка плагина | Th. 7, 16, 68, Cor. 69 | Обратное поставляет автор; рантайм **не проверяет**, что оно действительно откатывает: «an obligation on the component author rather than a property the runtime verifies» (§5.1.1). Коммутативность операций ключа тоже не проверяется. |
| Hot reload | HMR есть «выгрузить старый fiber, вставить новый» (§5.2.2); корректность итогового состояния даёт Th. 80 | Гарантируется состояние покоя, а не то, что произошло по дороге. Приватное состояние модуля теряется. |
| Порядок зависимостей | Th. 70, 73 | Цикл зависимостей оставляет участников навсегда неактивными (§6.5); предсказуемо по объявлениям. |
| Изоляция | realms (Def. 24), interception (Def. 26) | Это изоляция привязок, а не безопасность. §6.3: «Cordis supports the first [контроль доступа] through dependency declarations and interception; the second [sandbox] requires an external sandbox». |
| Граница системы | §6.1 | Внутри границы только то, что система меняет эксклюзивно и умеет восстановить. Отправленный пакет, письмо, платёж лежат снаружи: их можно только задержать или компенсировать, и метатеория на компенсации не распространяется. |
| Совместимость интерфейсов | §6.6 | Связывание только по имени ключа. Дрейф интерфейса и коллизии ключей названы открытой проблемой. |

Проверка на практике, по словам самой статьи, ограничена: кейс один (Koishi, более 4000 плагинов, причём на Cordis v3, а статья описывает v4), он наблюдательный, без измерений накладных расходов (§5.3). Применение к самоизменяющимся агентным harness в заключении названо **направлением будущей проверки**, а не результатом (§8). DeepSeek Harness в статье как кейс не разбирается.

### 1.4. Теория категорий: чья это рамка

Не статьи. В §2 сказано: «We assume familiarity with basic type theory and category theory», и там же как фон пересказаны монады Моджи и комонады для классических (статических) систем эффектов и коэффектов. Собственный аппарат статьи другой: моноиды и их гомоморфизмы, зависимые типы (свидетель как компонент Σ-типа), рекурсивные типы, операционная семантика с правилами вывода, наблюдательная эквивалентность, теория трасс. Слово «functor» в тексте не встречается ни разу; есть две коммутативные диаграммы. Раздел «Related Work» прямо отделяет Cordis от монадических систем эффектов: отслеживание идёт поверх обычного кода, а не внутри типа-монады (§7.1).

Описания «плагин как стрелка, композиция через функторы и монады» принадлежат блогам. Пример по поисковой выдаче: besthub.dev, «How DeepSeek Harness Uses Category Theory to Build a Robust AI Agent Skeleton» (не открывалась, не проверено). Более аккуратный разбор (redreamality.com, 2026-08-23) прямо пишет: «You do not need category theory to use it».

Вывод для нас: переносить нужно не категорный словарь, а четыре вещи: эффект возвращает своё обратное; аккумулятор обратных на владельца; committed view против target view; охранное условие на выгрузку провайдера.

## 2. Ядро Cordis: абстракции и реальный код

Ядро маленькое: `vendor/cordis/src` занимает около 2900 строк, loader около 1240, include около 1000, hmr около 460 (подсчёт по клону). Соответствие теории и кода дано в таблице 2 статьи: `Γ∞` это `ctx`, `effect` это `ctx.effect(callback)`, `d` это `fiber.inject`, `ω` это `fiber.committed`, аккумулятор это `fiber.dispose`.

### 2.1. Context

Контекст есть хранилище сервисов и точка всех регистраций. В JavaScript это `Proxy`: чтение `ctx.tools` перехватывается и разрешается через реестр. Дочерние контексты создаются прототипным наследованием и не меняют родителя.

```ts
// vendor/cordis/src/context.ts
constructor() {
  this[symbols.isolate] = Object.create(null)
  this[symbols.intercept] = Object.create(null)
  const self = new Proxy<this>(this, ReflectService.handler)
  this[symbols.root] = self
  this.fiber = new Fiber(self, {}, Object.create(null), null, () => [])
  this.reflect = new ReflectService(self)
  this.registry = new RegistryService(self)
  this.events = new EventsService(self)
  this.logger = new LoggerService(self)
  ...
}
```

Доступ к сервису без объявления зависимости есть ошибка времени выполнения. Разрешение идёт вверх по цепочке fiber: сначала собственные предоставления, затем committed view, иначе отказ.

```ts
// vendor/cordis/src/reflect.ts, обработчик Proxy
return ctx.events.waterfall('internal/get', ctx, prop, error, () => {
  const result = locate(defSite, prop)
  if (!result) throw error                     // "cannot get property ... without inject"
  if (result.impl) return getTraceable(ctx, result.impl.value)
  error.message = `cannot get required service "${prop}" in inactive context`
  throw error
})
```

### 2.2. Plugin

Плагин есть функция `(ctx, config)`, класс с конструктором `(ctx, config)` или объект с методом `apply`. Метаданные: `name`, `inject`, `provide`, `Config` (схема), `intercept` (`vendor/cordis/src/registry.ts`, `Plugin.Base`).

```ts
// docs/cordis-tutorial/03-services.md
export const name = 'consumer'
export const inject = ['greeter']

export function apply(ctx: Context) {
  console.log(ctx.greeter.greet('world'))
}
```

`ctx.plugin(plugin, config)` создаёт fiber и возвращает его; fiber можно ждать (`await`) и освобождать (`fiber.dispose()`).

### 2.3. Service

Сервис есть именованная возможность на `ctx`. Базовый класс `Service` регистрирует экземпляр в конструкторе, и эта регистрация сама является эффектом:

```ts
// vendor/cordis/src/service.ts
constructor(protected ctx: Context, name: string) {
  ...
  self.ctx.reflect.provide(name, self, this[symbols.check])
  return self
}
```

```ts
// docs/cordis-tutorial/03-services.md
declare module '@deepseek-ai/cordis' {
  interface Context { greeter: GreeterService }
}
export class GreeterService extends Service {
  constructor(ctx: Context) { super(ctx, 'greeter') }
  greet(who: string) { return `Hello, ${who}!` }
}
```

Типизация `ctx.greeter` достигается слиянием деклараций TypeScript (`declare module`); во время выполнения это ничего не порождает. Второй одноимённый провайдер в той же области отвергается: `service "X" has been registered at <...>` (`reflect.ts`, `provide`).

### 2.4. inject и реактивные зависимости

`inject` не разовая проверка при старте. Каждое изменение сервиса вызывает `notify`, тот пересчитывает «эпоху» каждого зависимого fiber: строку из номера ревизии и `uid` реализаций всех объявленных сервисов. Сменилась эпоха — fiber выгружается и, если зависимости снова на месте, загружается заново.

```ts
// vendor/cordis/src/fiber.ts
_refresh() {
  let epoch = `${this._revision}`
  for (const name of Object.keys(this.inject)) {
    const impl = this._store[name]
    if (!impl) { epoch = INACTIVE; break }
    epoch += ':' + impl.uid
  }
  this._setEpoch(epoch)
}
```

Сравнивается идентичность провайдера, а не значение: заменённый провайдер с равным значением всё равно перезапускает зависимых (в статье это обосновано в §4.2.2 и §5.1.3). Необязательная зависимость читается через `ctx.get('name')` без объявления в `inject`.

### 2.5. Effects и освобождение

Единственный примитив изменения контекста: `ctx.effect(() => disposer)`. Тело выполняется сразу, возвращённая функция выполняется при выгрузке владельца. Допустимые формы результата (`fiber.ts`, тип `Effect`): функция, `Promise` функции, синхронный или асинхронный итератор функций. Форма с генератором и есть effect iterator из статьи.

```ts
// docs/cordis-tutorial/02-lifecycle-and-effects.md
ctx.effect(() => {
  const timer = setInterval(() => console.log('tick'), 200)
  return () => {
    clearInterval(timer)
    console.log('heartbeat cleaned up')
  }
})
```

```ts
// vendor/cordis/src/reflect.ts: mixin как генератор, каждый yield отдаёт очередной disposer
return this.ctx.fiber.effect(function* () {
  ...
  for (const [key, value] of entries) {
    yield self.accessor(value, { get(receiver, error) { ... }, set(value, receiver, error) { ... } })
  }
}, `ctx.mixin(${JSON.stringify(source)})`)
```

Все встроенные регистрации уже эффекты: `ctx.on`, `ctx.plugin`, `ctx.provide`, `ctx.accessor`, а в dsh ещё `ctx.tools.register`, `ctx.llm.registerAdapter` и остальные реестры. Правило из `docs/cordis-primer.md`: «Every registration should have a disposer».

Детали, важные при переносе:

- Disposer одноразовый: повторный вызов ничего не делает (`fiber.ts`, `effect`). В статье объяснено почему: второй вызов применил бы обратное в состоянии, где оно ничего не обязано откатывать (§5.1.1).
- Disposers одного эффекта выполняются в обратном порядке и последовательно. Эффекты одного fiber при выгрузке освобождаются **параллельно** (`_unload`: `Promise.all(this._disposables.clear().map(...))`). Если шаги teardown должны идти по порядку, их держат в одном эффекте (`docs/cordis-tutorial/02`).
- Создавать эффекты в состоянии `UNLOADING` запрещено: `CordisError('INACTIVE_EFFECT')`.
- Ошибка в disposer логируется и не прерывает остальные.

### 2.6. Жизненный цикл fiber

```ts
// vendor/cordis/src/fiber.ts
export const enum FiberState { PENDING, LOADING, ACTIVE, FAILED, DISPOSED, UNLOADING }
```

`PENDING` ждёт зависимостей и не является ошибкой. `FAILED` выходит из этого состояния только через `fiber.update()` (перезапуска «самого по себе» нет, как и в §4.4 статьи). Переходы инертны: пока идёт `inertia` (обещание текущего перехода), новая цель только запоминается, а по завершении `_reload` цепляется `_unload` и наоборот.

Ключевое место, реализующее охранное условие и Теорему 70: провайдер ждёт, пока уйдут зависимые, и только потом освобождает свои эффекты, а зависимый до конца своего teardown держит committed view.

```ts
// vendor/cordis/src/fiber.ts
private async _unload() {
  // Dependents are notified when this fiber leaves ACTIVE, but may still
  // access its services during their own teardown, so the effects of this
  // fiber are only disposed after every dependent has unloaded.
  while ([...this.provisions].some(impl => impl.dependents.size)) {
    await new Promise<void>((resolve) => this._release = resolve)
  }
  ...
  await Promise.all(this._disposables.clear().map(async (dispose) => { ... }))
  ...
  this.committed = undefined
}
```

### 2.7. Области: extend, isolate, intercept, вложенные fiber

```ts
// vendor/cordis/src/context.ts
isolate(name: string, label?: symbol) {
  const shadow = Object.create(this[symbols.isolate])
  shadow[name] = label ?? Symbol(name)
  return this.extend({ [symbols.isolate]: shadow })
}
```

`isolate` даёт поддереву собственный экземпляр сервиса под тем же именем; одинаковая метка объединяет области. `intercept` добавляет поддереву метаданные доступа к сервису, которые провайдер читает при вызове (`Service[resolveIntercept]`). Понятия «fork» в Cordis v4 нет: дерево образуют вложенные вызовы `ctx.plugin` (у каждого fiber свой `ctx = parent.extend({ fiber })`) и группы загрузчика. В dsh слово «fork» относится к сессиям и субагентам.

Пример изоляции из поставки dsh: plan mode живёт в группе с собственным realm (`packages/bundle/web-app/presets/standard.patch.yml`):

```yaml
- id: planning
  name: cordis:group
  group: true
  isolate:
    planMode: true
  config:
    - id: plan-mode
      name: '@deepseek-ai/dsh-plan-mode'
```

Поверх этого dsh добавляет библиотеку `packages/core/scope`: наборы регистраций на одного агента с общим временем жизни (`agent.ctx`).

### 2.8. События

Пять режимов диспетчеризации (`docs/cordis-primer.md`, `vendor/cordis/src/events.ts`): `emit` (синхронно, без результата), `parallel` (все слушатели параллельно, ожидается), `serial` (по очереди до первого значимого результата), `bail` (синхронно до первого значимого результата), `waterfall` (middleware вокруг `next()`). Режим есть часть контракта события.

```ts
// vendor/cordis/src/events.ts
waterfall(...args: any[]) {
  const [thisArg, callbacks] = this._resolve('waterfall', args)
  const inner = args.pop()
  const dispatch = () => {
    const callback = callbacks.shift()
    if (!callback) return inner()
    let called = false
    const next = () => {
      if (called) throw new Error('next() called multiple times')
      called = true
      return dispatch()
    }
    return Reflect.apply(callback, thisArg, [...args, next])
  }
  return dispatch()
}
```

```ts
// docs/cordis-tutorial/07-into-the-harness.md: плагин-наблюдатель
export const inject = ['tools']
export function apply(ctx: Context) {
  ctx.on('tools/result', (exec, result) => { ... })
}
```

Сам жизненный цикл открыт через события `internal/*`: `internal/plugin`, `internal/status`, `internal/service`, `internal/update`, `internal/config`, `internal/get`, `internal/set`, `internal/listener`, `internal/dispatch` (`events.ts`, интерфейс `Events`).

### 2.9. Loader: декларативная конфигурация

Запись конфигурации (Def. 81 статьи; `vendor/loader/src/config/entry.ts`): `id`, `name` (или URL модуля), `config`, `disabled`, `isolate`, `intercept`. Группы (`cordis:group`) и включения внешних файлов (`include`) сами являются плагинами. При изменении записи loader выбирает наименее разрушительное действие: смена `id` или модуля пересобирает запись, `config` передаётся плагину (событие `internal/update`, плагин решает, перезапускаться ли), `intercept` обновляется на месте, `disabled` выгружает и загружает (§5.2.1). Порядок строк не задаёт порядок загрузки: «activation is service-availability driven» (комментарий в `packages/bundle/base/cordis.patch.yml`).

## 3. Как dsh реализует «everything is a plugin»

### 3.1. Что является плагином

Всё, включая цикл агента. `docs/architecture.md`: «There is no privileged core to patch: you extend dsh by mounting a plugin beside the others, and registrations are effects that unwind when their plugin unloads».

| Роль | Пакеты (примеры) | Ключ на `ctx` |
|---|---|---|
| Журнал сессии | `core/session` | `ctx.sessions` |
| Сборка system prompt и схем инструментов | `core/system-prompt` | `ctx.systemPrompt` |
| Реестр инструментов и охраняемый конвейер исполнения | `core/tools` | `ctx.tools` |
| Интерфейс `Agent`, живой реестр, события `agent/*` | `core/agent` | `ctx.agents` |
| Цикл агента (заменяемый драйвер) | `core/agent-loop` | `ctx.agentLoop` |
| Модели | `llm/llm` + адаптеры | `ctx.llm` |
| Файловая система | `fs/fs`, `fs-local`, `fs-sandbox` | `ctx.fs` |
| Оболочка и процессы | `shell/*`, `subprocess/*` | `ctx.shell`, `ctx.subprocess` |
| Sandbox и разрешения | `sandbox/*`, `interaction/user-approval`, `permission-presets` | `ctx.sandbox`, `ctx.sandboxPolicy`, `ctx.approval` |
| Хранение | `session-persistence-jsonl`, `storage/*` | `ctx.sessionPersistence`, `ctx.storage` |
| Расписание, фоновые задания | `schedule/*`, `jobs/*` | `ctx.jobs` и др. |
| Субагенты | `subagent/*` | `ctx.subagents` |
| Web UI | `host/webserver`, `api/*`, `client/*` | `ctx.typertGateway`, клиентские слоты |
| Сама загрузка | `boot/hmr`, `boot/plugin-manager`, `boot/config-editor` | `ctx.hmr`, `ctx.pluginManager` |

Таблица «Core packages» и таблица «Where new behavior goes» находятся в `docs/architecture.md`.

Организующий приём называется **capability seam**: каждая заменяемая возможность разделена на три роли: Service Definition (интерфейс), Service Provider (реализация), Consumer (обычно инструмент для модели). `packages/README.md`: «Extension plugins depend on Service Definitions, never concrete providers». Следствие, заявленное в `docs/architecture.md`: замена провайдера fs и subprocess на удалённый sandbox переносит Bash, PTY и LSP без правки инструментов.

Точки расширения делятся на три домена событий (`docs/architecture.md`, «Events»):

- события сессии: устойчивые факты в журнале (`turn/*`, `step/*`, `user/message`, `assistant/message`, `tool/*`);
- события агента `agent/*`: живое наблюдение и перехват (`agent/pre-step`, `agent/request`, `agent/turn-stopping`, `agent/assistant-stream`);
- события возможностей: `fs/*`, `tools/*`, `telemetry/*`.

Правило «Model-visible means logged»: любой вход модели должен восстанавливаться из журнала сессии.

### 3.2. Раскладка и слои конфигурации

420 пакетов (подсчёт `package.json` в клоне). Группы перечислены в `packages/README.md`; полная опись со строками кода дана в разделе 7.1 и приложении A. Все пакеты имеют префикс `@deepseek-ai/dsh-*`.

Запущенный `dsh` есть дерево плагинов, собранное при старте из упорядоченных слоёв (`docs/architecture.md`, `packages/boot/app-boot/README.md`):

1. **Bundle**: формат поставки строк конфигурации и кода, который они монтируют. Объявляется в `package.json` полем `dsh.bundle` (путь к файлу патча).
2. **Profile**: именованная композиция в `$DSH_HOME/profiles/<name>`: список bundles (`dsh.profile.bundles`), установленные внешние плагины, собственный `cordis.patch.yml`. Поставляются шаблоны `web`, `headless`, `sdk`, `sdk-minimal`, `acp`.
3. Порядок слоёв: bundles профиля, затем `cordis.patch.yml` профиля, затем патч домашнего каталога, затем `--patch` из командной строки. Патч адресует строку по `id` и заменяет её `config` целиком либо вставляет новые строки.
4. **Agent preset**: декларативный набор плагинов на одну сессию (`packages/preset/agent-preset`). В Web-профиле их четыре: `standard`, `cordis` (режим Creator), `ptc`, `minimal` (`packages/bundle/web-app/presets/*.patch.yml`).

`dsh-base` есть общий первый слой (99 строк с полем `name` в `packages/bundle/base/cordis.patch.yml`; 87 пакетов монтируются безусловно, остальные строки условны через `!!js` или выключены): адаптеры моделей, инструменты, персистентность, sandbox и approval, настройки, учётные данные, телеметрия. Поверх него `dsh-web-app` добавляет браузерное приложение, `dsh-headless` одноразовый запуск, `dsh-sdk-app` сервер JSON-RPC, `dsh-acp-app` сервер ACP. `dsh-sdk-minimal` намеренно не использует base и содержит полное явное дерево.

Посмотреть итоговое дерево: `dsh --profile web --dump-config`.

### 3.3. Как третья сторона расширяет dsh

| Цель | Механизм (`docs/architecture.md`) |
|---|---|
| Провайдер моделей | зарегистрировать адаптер на `ctx.llm` |
| Инструмент для модели | `ctx.tools.register(defineTool({...}))` |
| Другой набор возможностей для одной сессии | собрать agent preset |
| Исполнение в оболочке | бэкенд `ctx.shell` |
| Перехват запроса, инструмента, хода | события `agent/*`, `tools/*` |
| Контекст для модели | `agent.inject()` |
| Устойчивое состояние сессии | расширить `SessionEventMap` |
| Хранение сессий в другом бэкенде | реализовать `SessionPersistence` |
| Элемент Web UI | `ConversationNodeDefinition` и рендерер, слоты клиента |
| Внешний инструмент без кода | строка `@deepseek-ai/dsh-mcp-client` (MCP) |

Минимальный внешний плагин из шаблона, поставляемого для режима Creator (`packages/preset/agent-preset/skills/dsh-doc/templates/`): обычный JavaScript без сборки.

```json
// templates/decoration/package.json (сокращено)
{
  "name": "@local/my-decoration",
  "type": "module",
  "exports": { ".": "./index.js", "./client": "./client.js" },
  "dsh": { "bundle": { "patch": "./cordis.patch.yml" } }
}
```

```yaml
# templates/mcp/cordis.patch.yml: bundle без кода, только конфигурация
- insert:
    - id: demo-mcp
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: demo
        transport: streamable-http
        url: http://127.0.0.1:3000/mcp
```

### 3.4. Обнаружение, установка, загрузка

- Плагин есть npm-пакет. Установка идёт через Plugin Manager (`packages/boot/plugin-manager`): он вызывает pnpm в каталоге профиля, принимает имя из реестра, git-адрес, tarball или абсолютный путь, проверяет совместимость (`engines.dsh` или peer-диапазоны), ведёт `package.json` профиля и `pnpm-lock.yaml`, при неудаче восстанавливает оба файла. Требуется pnpm в `PATH` (или встроенный в Desktop).
- Обнаружение: тема GitHub `dsh-plugin` (`README.md`; это самоназначаемая метка, не проверяемый реестр) и индексы плагинов, которые настраивает пользователь (`packages/boot/plugin-market`: вкладка Discover и инструмент `plugin_market`).
- Те же операции доступны модели через инструмент `plugin_manager`. Каждое действие требует режима `danger-full-access` либо разового подтверждения: «installed Host code executes in-process outside the workspace sandbox» (`packages/boot/plugin-manager/README.md`).
- Проверки совместимости прямо названы не защитой: «These checks are not a sandbox against malicious package code» (`packages/boot/app-boot/README.md`).

### 3.5. Работает ли hot reload на практике

Нужно различать три вещи.

**Перекомпоновка конфигурации** работает и включена по умолчанию в Web-профиле. Base монтирует `dsh-hmr` с `root: []` (строка условная: `disabled: !!js "!ctx.get('profileContext')"`), то есть только слежение за манифестом профиля и двумя пользовательскими патчами (`packages/bundle/base/cordis.patch.yml`). Изменение строки приводит к выгрузке и загрузке только затронутых плагинов. Headless, SDK и ACP HMR отключают: там изменения применяются при перезапуске (`docs/architecture.md`).

**Замена кода модулей** существует, покрыта тестами (`apps/cli/tests/profile-hmr.spec.ts`, `apps/cli/tests/built-bin.e2e.ts`), но держится на внутренностях загрузчика Node и имеет длинный список ограничений (`packages/boot/hmr/README.md`, «Known Limitations»):

- «Module replacement requires Node loader internals»;
- замена идёт по статическим импортам; динамический `import()` устаревшего модуля, существующие Workers и поток TSX-загрузчика держат старый кэш;
- изменения в хост-зависимостях (сам Cordis, Loader, app-boot) требуют перезапуска процесса;
- при неудачной замене в части путей «the previous version is not restored»: плагин остаётся неактивным до исправления файлов;
- в упакованном worker-рантайме замена кода недоступна;
- есть гонки с временем модификации файлов.

Замена уже установленного пакета без HMR требует перезапуска (`packages/boot/plugin-manager/README.md`, «Known Limitations»).

**Позиция самих сопровождающих**: в предложении `.agents/notes/proposed/simplification/2026-09-19-config-only-hmr.md` (статус «proposed») предлагается оставить только перекомпоновку конфигурации, а замену исходных модулей убрать, потому что она тянет «Node-loader internals, dependency traversal, cache backup, old/new plugin fibers, rollback, module watches». Там же оценка: около 400 строк исходников и 372 строки тестов уходят.

Отдельно: HMR не переносит состояние. Приватное состояние модуля при перезагрузке сбрасывается; долговечное состояние должно жить в сервисе с более длинным временем жизни или в журнале сессии.

Вывод: гарантию «изменение состава и настроек без перезапуска» dsh держит уверенно. Гарантию «замена кода на лету» сам проект считает дорогой и хрупкой.

### 3.6. dsh как «harness над harness»

Это уже есть в ограниченном виде, и это прямо касается нашего продукта. Группа `packages/subagent` содержит провайдеров, запускающих чужие продукты как субагентов (`packages/subagent/README.md`; проектная заметка `.agents/notes/archived/feature/2026-08-04-claude-code-and-codex-subagent-backends.md`):

| Провайдер | Как запускает |
|---|---|
| `subagent-claude-code` | официальный `@anthropic-ai/claude-agent-sdk` (закреплён 0.3.263), функция `query()`; CLI берётся из платформенного пакета SDK, а не из `PATH` |
| `subagent-codex` | закреплённый `@openai/codex` 0.153.4, `codex app-server --stdio`, эфемерный thread, один `turn/start` |
| `subagent-acp` | любой агент, говорящий по Agent Client Protocol: конфигурация `command`, `args`, `env`, `permission` |
| `subagent-dsh-sdk` | другой экземпляр dsh через JSON-RPC SDK |

Ограничения, зафиксированные в заметке: каждый вызов есть свежий процесс и невозобновляемый разговор; провайдеры «do not resume sessions, stream progress, accept new human interaction, roll back tool or file side effects, or impose a wall-clock timeout»; запросы разрешений отвечаются политикой без человека; от родителя передаются только текст задачи и рабочий каталог. Для нашего продукта, где чужой harness должен быть основным исполнителем, а не одноразовым помощником, этой модели недостаточно, но устройство слоёв (seam `ctx.subagents`, общий владелец дерева процессов `dsh-subprocess`, очистка учётных данных из окружения) стоит перенять.

## 4. Задачи, skills, память: есть ли адаптация под задачу

Коротко: механизма «новый вид задачи превращается в плагин с источниками, инструментами и памятью» в dsh нет. Есть четыре соседних механизма.

**Skills** (`packages/skill/*`). Переиспользуемые инструкции под задачу, загружаемые по требованию: реестр `ctx.skills` объединяет каталоги из файловых каталогов, встроенных данных плагинов и удалённых сервисов; модель видит каталог и подгружает полный текст инструментом `skill` (параметр `name`) либо пользователь вызывает `/name`. Формат: каталог с `SKILL.md` (frontmatter `name`, `description`) и файлами `references/`, `templates/`. Это статические инструкции: skill не регистрирует инструменты и не хранит память.

**Режим Creator и Plugin Manager**. Агент пишет пакет плагина и YAML-патч как обычные файлы в workspace и устанавливает их вызовом `plugin_manager` с действием `install_bundle`. Знания о том, как писать плагины, поставляются skill `dsh-doc` со справочниками и шаблонами (`packages/preset/agent-preset/skills/dsh-doc/`). Для самоинспекции есть два инструмента только на чтение: `cordis_inspect_list` и `cordis_inspect_query` (`packages/extensions/tool-cordis`).

Здесь важна история. Первая версия давала модели инструменты define/run/stop/undefine: сгенерированный код исполнялся в `node:vm` как процессно-локальное «динамическое определение». От этого отказались (`.agents/notes/implemented/architecture/2026-09-16-creator-persistent-plugin-management.md`):

- «Generated-code tools create a second plugin lifecycle alongside ordinary installed bundles»;
- определения исчезали при перезапуске, а воссоздавать их из журнала сессии нельзя: «replaying source would execute historical side effects»;
- vm не является защитой: «The sandbox isolates globals but is not a security boundary… Treat a dynamic package like bash access» (`packages/extensions/cordis-host-runner/README.md`).

Итоговое решение: один жизненный цикл (обычные установленные bundles), явное подтверждение на каждое изменение профиля, изменения переживают перезапуск и действуют на все сессии профиля.

**Personalized pages** (`packages/experimental/creation-recommendations`, экспериментальное, по умолчанию выключено). Ближайший аналог нашей идеи. Если у результата есть «concrete continuing use», агент после ответа предлагает карточку; по нажатию Create фоновый субагент-Creator пишет новый плагин в `.dsh-created-plugins/<id>`, хост проверяет пакет и патч (только вставка собственных уникально названных строк, без YAML-тегов и включений), копирует в каталог профиля и устанавливает. Undo удаляет плагин, Restore ставит сохранённую копию без повторной генерации. Ограничения: создаются страницы и панели интерфейса, а не «рельсы» задачи; одна карточка на ход; только при живом корневом разговоре и праве менять профиль; память и источники не моделируются.

**Память**. Встроенной долговременной памяти агента нет. Решение записано в `.agents/notes/archived/feature/2026-07-31-third-party-memory-mcp-examples.md`: вместо собственной интеграции поставляются три выключенных примера подключения сторонних MCP-серверов памяти (Memorix, MCP Reference Memory, Engram); «There is no memory preset registry, vendor-specific DSH plugin, universal memory service». Рядом есть:

- `context/agent-instructions`: чтение `AGENTS.md`-совместимых файлов (пользовательских и проектных) в контекст;
- `session-query` и экспериментальный `tool-session-query`: полнотекстовый поиск по прошлым сессиям (SQLite), доступ модели ограничен сессиями с тем же `cwd`;
- `goal`: одна устойчивая цель на сессию; `schedule`: повторяющиеся задачи по RFC 5545; `todo`; `workflow` (скрипт оркестрации субагентов); экспериментальный `tool-ralph` (последовательность свежих агентов над одной целью).

Заметка с названием `session-reading-memory` относится к отметкам «прочитано/не прочитано» в интерфейсе, а не к памяти агента.

В предложениях (статус «proposed») есть `2026-08-04-task-surface.md` и `2026-10-29-event-triggered-automation-tasks.md`; они не читались подробно.

Что из этого следует для «каждая задача есть плагин»:

1. Форма «bundle = данные + необязательный код, устанавливается транзакционно, отменяется одним действием» в dsh уже отработана. Её можно взять как форму task-плагина.
2. dsh сознательно ушёл от исполнения сгенерированного кода с отдельным жизненным циклом. Наш task-плагин должен быть обычным плагином ядра с тем же циклом, а не особым видом объекта.
3. Память под задачу и поиск источников в dsh не решены. Это наша собственная часть, прецедента в dsh нет.

## 5. Критика и известные ограничения

### 5.1. Что признаёт сам проект

- `SAFETY.md`: «experimental developer-preview software. It has not undergone a security audit and must not be treated as secure or production-ready»; sandbox, подтверждения и права «do not guarantee isolation or prevent damage».
- Плагины исполняются в процессе хоста с его правами; установка плагина приравнена к доступу к оболочке.
- Обратные действия и коммутативность не проверяются рантаймом (§5.1.1 статьи).
- Sandbox «same-world»: общие с хостом ядро ОС и файловая система; сеть и видимость процессов вне гарантий (`packages/sandbox/README.md`, `packages/shell/bash-sandbox/README.md`). На Windows гарантия частичная (`packages/sandbox/sandbox-windows-acl/README.md`).
- Вопрос версий интерфейсов сервисов не решён (§6.6 статьи).
- Четыре опубликованных разбора инцидентов (`docs/postmortem/`). Показателен 0001: сервер ACP падал на первом же запросе с ошибкой `cannot get property "agents" without inject`, потому что лишний `export default apply` отбрасывал объявление `inject` у плагина; 178 зелёных unit-тестов и 100 % покрытия это пропустили, так как тесты монтировали плагин не тем путём, каким он грузится в продукте. Разбор 0002: выражение `!!js` в поле `disabled` всегда считалось истинным, и файловые инструменты были постоянно отключены, а снимочные тесты закрепили регрессию как ожидаемый результат. Обе ошибки происходят из динамичности JavaScript и YAML с выражениями; в Rust первая стала бы ошибкой компиляции.

### 5.2. Внешняя критика

| Источник | Суть | Статус проверки |
|---|---|---|
| besthub.dev, «Beyond 'Everything Is a Plugin': Defining Production Responsibility in Agent Harness», 2026-09-15, пересказ статьи из серии | Обратимость рантайма не равна обратимости последствий: `dispose()` чистит локальные ресурсы, а опубликованные конфигурации, уведомления и трафик остаются. Нужны контракты действий (кто вправе, предусловия, доказательство успеха), журнал сборки возможностей, привязка к версии и дайджесту плагина, повторная проверка старых свидетельств при смене состава; агент не должен сам находить и ставить плагины из произвольных источников; «completed» должно выводиться правилами из свидетельств. По пересказу, статья не называет DeepSeek Harness по имени и критикует подход в целом. | Прочитан пересказ на besthub; оригинал за входом |
| redreamality.com, «Cordis Explained…», 2026-08-23 | Доказательства не проверены машинно; Koishi работает на Cordis v3; нет измерений; disposers не проверяются на честность; посредничество контекста не изолирует враждебный код («Plain Node.js code can bypass proxies»); HMR не есть миграция состояния; ключ сервиса не гарантирует совместимость контракта; официальной реализации Cordis на Rust не найдено | Прочитано |
| Habr, статья 1070958, автор profleaddev, 16 августа | Обзорный текст на английском; теории категорий и статьи не упоминает. Риски: меняющиеся форматы конфигурации и API плагинов, неполная документация, зависимость предположений безопасности от ОС, малая история эксплуатации, нестабильная экосистема плагинов | Прочитано; комментариев 0 |
| securityarsenal.com: агент может отключить собственный файловый sandbox | Единственный вторичный источник, CVE нет | Не проверено |
| implicator.ai, wavect.io, hyrax.dev | Плагины работают с правами пользователя; preset есть код с правами загружаемых плагинов; каталог проверяет установку, а не безопасность | Только поисковая выдача, не проверено |

### 5.3. Ограничения, существенные для переноса

1. Гарантии теории условны: честные обратные, коммутативные ключи, ацикличный граф, всё общее состояние заведено под ключи. Нарушение не обнаруживается.
2. Внешние эффекты (сеть, файлы вне эксклюзивного владения, сообщения) теорией не покрываются. Для агента, который в основном действует вовне, это главная часть работы.
3. Изоляции от недоверенного кода нет ни в Cordis, ни в dsh.
4. Замена кода на лету привязана к Node и самим проектом предлагается к удалению.
5. Отладка: плагин с невыполненной зависимостью молча ждёт в `PENDING` (`docs/cordis-tutorial/06`). Нужна диагностика «кто чего ждёт».
6. Размер: 663 тыс. строк исходников и 908 тыс. строк тестов; из них клиент Web UI 218 тыс., экспериментальное 142 тыс. (раздел 7.1).

## 6. Перенос в Rust

### 6.1. Что в Cordis опирается на динамичность JavaScript

| Свойство в JS | Где используется | Как получить в Rust |
|---|---|---|
| `Proxy` на контексте: `ctx.tools` разрешается при чтении, с проверкой `inject` | `vendor/cordis/src/reflect.ts` | Статья сама называет замену (§6.4): трейты для типизации ключей и процедурные макросы, порождающие типизированный accessor на каждую зависимость. Практически: `#[derive(Inject)] struct Deps { tools: Service<dyn Tools> }`, а fiber получает уже разрешённую структуру. Недекларированный доступ становится невозможен на уровне типов. |
| Слияние деклараций (`declare module`), открытый `Context` | все пакеты dsh | Ключ сервиса как тип-маркер с ассоциированным типом значения (`trait ServiceKey { type Value: ?Sized; const NAME: &str; }`); реестр хранит `Arc<dyn Any + Send + Sync>` по `(TypeId, realm)`. |
| Динамический `import()` и вытеснение из кэша модулей | `vendor/loader`, `vendor/hmr` | Для собственных плагинов не нужно: код слинкован статически, динамичен состав (таблица «имя → конструктор»). Для чужого кода: процессы, WASM, встроенный интерпретатор (6.3). Статья (§6.4) прямо называет для нативного кода `dlopen/dlclose`, а для WASM: экземпляр освобождается, когда встраивающий хост его отпускает (Wasmtime). |
| Сборщик мусора: замыкания-disposers свободно захватывают состояние; старый экземпляр сервиса живёт, пока на него ссылаются | везде | `Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>` с явным владением. Сервисы за `Arc`. Committed view есть набор `Arc`, который fiber держит до конца своего teardown: это даёт Теорему 70 буквально. Преимущество Rust: после выгрузки провайдера можно проверить `Arc::strong_count` и обнаружить утечку ссылки, чего JS сделать не может. |
| Promise с немедленным планированием, однопоточный цикл событий: «проверил и изменил» атомарно между `await` | `fiber.ts` (`inertia`, `_setEpoch`) | Статья отмечает это в сноске к алгоритму 5: при ленивых future (Rust) хост обязан явно порождать задачу. Ядро делаем актором: один владелец реестра и состояний fiber (задача на `LocalSet` или выделенный поток), все изменения идут сообщениями. Плагины могут быть многопоточными, решения о жизненном цикле нет. |
| Асинхронное освобождение: disposer возвращает Promise | `fiber.ts` (`_unload`) | Асинхронного `Drop` в стабильном Rust нет: в дорожной карте 2026 «Just add async» деструкторы названы только синхронными, работа над async drop продолжается (https://rust-lang.github.io/goals/2026/roadmap-just-add-async.html). Освобождение делаем явным: ядро вызывает `dispose().await`. `Drop` оставляем как страховку, которая сообщает о не освобождённом эффекте. |
| Генераторы как effect iterator, прерывание между `yield` | `fiber.ts` (`_execute`) | `async fn apply(ctx)` с точками отмены: каждая регистрация `ctx.effect(...)` проверяет эпоху fiber и возвращает ошибку отмены. Либо явный список шагов. |
| Прототипное наследование контекстов (`extend`, `isolate`) | `context.ts` | Неизменяемые таблицы realm с общим хвостом (`Arc<RealmTable>` со ссылкой на родителя). |
| `!!js`-выражения в YAML конфигурации | `vendor/include`, `vendor/loader` | Не переносить. Заменить закрытым набором условий (платформа, наличие сервиса, переменная окружения). Разбор 0002 показывает цену выражений. |
| Утиная типизация событий по строковому имени | `events.ts` | Событие как тип с объявленным режимом диспетчеризации: `trait Event { type Args; type Output; const MODE: Dispatch; }`. Waterfall есть цепочка middleware с `Next`. |

### 6.2. Прецеденты в Rust

Критерии: (1) загрузка без пересборки хоста; (2) Windows; (3) изоляция; (4) может ли LLM написать такой плагин на машине без Rust toolchain; (5) зрелость.

| Подход | (1) Без пересборки | (2) Windows | (3) Изоляция | (4) LLM без toolchain | (5) Зрелость и свидетельства |
|---|---|---|---|---|---|
| **Bevy plugins** (`Plugin::build(&self, app)`) | Нет. Плагины компилируются в бинарник; выгрузки плагина нет | Да | Нет | Нет | Движок зрелый. Динамические плагины объявлены устаревшими в 0.14 «for being unsound» и удалены в 0.15 (PR #14534; https://bevy.org/learn/migration-guides/0-14-to-0-15/) |
| **rig-harness на rig-ecs** (`[rig]`) | Нет. Launcher генерирует Cargo-проект из `RIG_HOME/settings.toml`, собирает и запускает; `/reload` пересобирает и перезапускает в той же сессии | Не заявлена: «runs on Linux and macOS» | Нет (кроме Steel для code mode) | Нет: нужен Rust ≥ 1.97.1 и компиляция | Версия 0.46.0 от 2026-10-31; Bevy закреплён `=0.20.1` (`Cargo.toml` workspace). Источник: `crates/rig-harness/README.md`, `PLUGINS.md` |
| **WASM Component Model** (wasmtime, WIT) | Да | wasmtime: x86_64 Windows, Linux, macOS в Tier 1; aarch64 Linux и macOS в Tier 2; aarch64 Windows в Tier 3 (https://docs.wasmtime.dev/stability-tiers.html) | Сильная: компонент получает только импорты, выданные хостом; лимиты памяти и времени | Rust-компонент: нет. JS-компонент: только если на машине есть инструмент компоновки (ComponentizeJS ставится как библиотека Node.js, помечен «experimental», встраивает SpiderMonkey, около 8 МБ) | `component-model` и `component-model-async` в Tier 1 у wasmtime. WASI 0.3 выпущен 2026-06-11 по вторичным источникам |
| **Extism** | Да | Да (установщик компилятора JS-плагинов покрывает Linux, macOS, Windows) | Сильная (WASM), host functions по списку | JS-плагин: нужен `extism-js` и Binaryen (`wasm-merge`, `wasm-opt`) в `PATH`; исполнение синхронное, без цикла событий, ES2020 (https://github.com/extism/js-pdk) | crate `extism` 1.x, BSD-3; 5,8 тыс. звёзд |
| **Javy** (JS → WASM на QuickJS) | Да | Не проверено | Сильная (WASM) | Нужен только бинарник `javy`: `javy build index.js -o index.wasm`; с динамической линковкой модуль 1–16 КБ (https://github.com/bytecodealliance/javy) | Проект Bytecode Alliance, 2,8 тыс. звёзд |
| **Подпроцесс, JSON-RPC по stdio** (стиль MCP; также ACP, LSP, `codex app-server`) | Да | Да | Граница процесса; усиливается sandbox ОС | Да, если на машине есть интерпретатор (Python, Node) или плагин есть готовый бинарник | Самый распространённый путь: MCP, ACP; в `[dsh]` так подключены MCP-серверы, Claude Code, Codex, ACP-агенты, LSP |
| **Динамические библиотеки** (`abi_stable`, `libloading`) | Да | Не проверено по странице crate | Нет: код в адресном пространстве хоста | Нет: нужен toolchain и компиляция под платформу | `abi_stable` 0.11.3: «plugin systems (without support for unloading)» (https://docs.rs/abi_stable). Статья (§6.4) упоминает `dlopen/dlclose` как путь нативного кода, но выгрузка библиотек в Rust небезопасна |
| **Встроенный скриптовый язык** | Да | Да | Средняя: скрипт видит только то API, которое дал хост; нужны лимиты времени и памяти | Да. Качество зависит от языка: JavaScript и Python модели пишут уверенно, Rhai, Steel и Lua заметно хуже | Rhai (5,8 тыс. звёзд), Steel (2,7 тыс.; используется в `[rig]` как `rig-steel`: «Steel's sandboxed engine», лимиты `max_seconds`, `max_calls`, `max_output_bytes`), QuickJS через rquickjs (его же использует Extism js-pdk) |

Наблюдения:

- rig-harness есть прямое подтверждение, что «всё есть Bevy-плагин» в Rust работает для **статического** набора плагинов и требует toolchain для любого изменения состава. Для машины заказчика с пользователями базового уровня это неприемлемо. Полезная идея оттуда: каждый плагин оборачивается в `Loaded`, который записывает плагин как сущность (`PluginSource`) и помечает всё, что он добавил (`ProvidedBy`). Это тот же учёт владения, что и аккумулятор Cordis, но без выгрузки.
- ECS упорядочивает по стадиям расписания, Cordis по доступности сервисов. Для «выгрузить провайдера → зависимые свернулись → вернулись» модель Cordis подходит точнее.
- Официальной реализации Cordis на Rust нет (redreamality.com; собственный поиск тоже ничего не дал).

### 6.3. Рекомендация

**(a) Плагины ядра, которые поставляем мы: статический код, динамическая композиция.**

- Все наши плагины есть crates, слинкованные в один бинарник под каждую платформу. Реестр «имя → конструктор» (явная таблица или `inventory`/`linkme`).
- Собственное маленькое ядро по образцу Cordis, а не Bevy: context, fiber, effect с disposer, сервисы с realm, реактивный `inject`, события с пятью режимами, loader с записями (`id`, `name`, `config`, `disabled`, `isolate`) и слоями патчей. Объём образца: около 2,9 тыс. строк ядра и 1,2 тыс. строк loader на TypeScript.
- Переносим семантику полностью: состояния fiber, инертные переходы, committed view, ожидание зависимых перед выгрузкой провайдера, одноразовые disposers, запрет эффектов в `UNLOADING`, `FAILED` без самоперезапуска, перекомпоновку конфигурации без перезапуска процесса.
- Не переносим: замену кода на лету (сам dsh предлагает её убрать), `!!js` в конфигурации, npm как канал установки.
- Используем сильные стороны Rust: `inject` проверяется компилятором (класс ошибок разбора 0001 исчезает), утечки ссылок на выгруженный сервис обнаруживаются по счётчику `Arc`, свидетельство «disposer возвращён» можно сделать обязательным типом (`#[must_use]`).
- Проверяем соответствие по тестам upstream: `[cordis] packages/core/tests` (fiber, dispose, service-dispose, isolate, intercept, events, update, replace; около 4,3 тыс. строк) и `[dsh] scripts/cordis-*.spec.ts`.

**(b) Task-плагины, создаваемые на машине заказчика: три ступени, от данных к коду.**

Условия: toolchain нет; автор плагина есть локальная модель Qwen; пользователи базового уровня; aarch64 Linux (DGX Spark; архитектура не проверялась в этом исследовании).

1. **Декларативный bundle (по умолчанию).** Каталог с манифестом и данными: описание задачи и признаки её распознавания, инструкции в формате skill, секции промпта, список разрешённых инструментов, источники, пространство имён памяти, строки конфигурации готовых плагинов ядра (например, подключение MCP-сервера). Кода нет. Загружает его наш плагин ядра, каждая регистрация идёт через эффекты, поэтому выгрузка и отмена бесплатны. Прецеденты в dsh: skills, bundle «только конфигурация» для MCP, проверка «только вставка собственных строк» в creation-recommendations. Слабой модели проще всего написать именно это, а проверка сводится к схеме.
2. **Скриптовые инструменты во встроенном интерпретаторе JavaScript.** Когда задаче нужен собственный инструмент. Интерпретатор (QuickJS) поставляется внутри бинарника; скрипт получает узкий объект возможностей: файлы в пределах workspace, HTTP по списку разрешённых адресов, память задачи, вызов других инструментов через общий конвейер с разрешениями. Лимиты времени и памяти обязательны. Вариант с более сильной границей: тот же интерпретатор, заранее скомпилированный в WASM-компонент и исполняемый в wasmtime, скрипт передаётся как данные. Тогда импорты WIT становятся буквальным `inject`, а компиляция на машине заказчика не нужна. Цена: aarch64 Linux у wasmtime в Tier 2, aarch64 Windows в Tier 3.
3. **Внешние процессы по stdio.** MCP-серверы для готовых интеграций и ACP либо родные протоколы для чужих harness (Claude Code, Codex, OpenCode, Qwen Code). Это тот же механизм, которым продукт и так запускает чужие harness; граница процесса плюс sandbox ОС.

Не рекомендуется для task-плагинов: динамические библиотеки (toolchain, нет выгрузки, нет изоляции), компиляция Rust в WASM на месте (toolchain), пересборка хоста (модель rig-harness).

Уроки dsh, которые нужно встроить сразу:

- один жизненный цикл для всех плагинов, включая сгенерированные;
- установка транзакционна, с отменой и восстановлением сохранённой копии;
- сгенерированный плагин хранится на диске и переживает перезапуск; из журнала сессии код не воспроизводится;
- изменение состава затрагивает все сессии профиля, поэтому требует явного подтверждения человека.

## 7. Объём переписывания dsh на Rust

Этот раздел описывает, что именно пришлось бы воспроизвести, чтобы получить harness «по образу» dsh. Все числа получены подсчётом по клону `[dsh]` на указанном commit. Способ подсчёта: строки файлов в каталогах `src/` и `tests/` каждого пакета (пакет есть каталог с `package.json`). В счёт попадают сгенерированные файлы, поэтому числа завышают ручной код в нескольких местах (явно: `packages/extensions/tool-cordis`, где лежат сгенерированные каталоги API). Строки TypeScript служат мерой относительного размера, а не прогнозом объёма кода на Rust.

### 7.1. Опись пакетов по ролям

Колонки принадлежности показывают число пакетов группы, которые соответствующий слой монтирует строкой конфигурации безусловно: `base` это `packages/bundle/base/cordis.patch.yml`, `web-app` это `packages/bundle/web-app/cordis.patch.yml`, `standard` это preset `packages/bundle/web-app/presets/standard.patch.yml`, `sdk-minimal` это `packages/bundle/sdk-minimal/cordis.patch.yml`. Пакеты-библиотеки, которые импортируются кодом, но не монтируются строкой, в этих колонках не отмечены. Полная опись всех 420 пакетов с назначением и отметками «да / усл. / выкл.» дана в приложении A.

| Группа | Роль | Пакетов | Строк src | Строк тестов | В base | В web-app | В standard | В sdk-minimal |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| `vendor` | Ядро: Cordis (core, loader, include, group, hmr, timer, logger) + cosmokit, schemastery | 9 | 7484 | 0 | 1 |  |  | 1 |
| `packages/core` | Позвоночник агента: session, system-prompt, tools, agent, agent-loop, scope, executors | 10 | 14253 | 35117 | 7 |  |  | 6 |
| `packages/llm` | Модели: seam `ctx.llm`, адаптеры DeepSeek и pi-ai, retry, token-meter | 9 | 15556 | 27190 | 8 |  |  | 6 |
| `packages/session` | Журнал сессий: JSONL-персистентность, миграции формата, проекции, заголовки, рабочий каталог | 22 | 19057 | 33198 | 10 | 3 |  | 7 |
| `packages/session-query` | Поиск по истории сессий (SQLite FTS), экспорт | 3 | 6042 | 8564 | 1 | 1 |  |  |
| `packages/storage` | Несессионное хранилище (JSON/SQLite, типизированные домены) | 4 | 2634 | 2398 | 3 |  |  |  |
| `packages/fs` | Файловая система: seam, локальный провайдер, sandbox-обёртка, инструменты read/write/edit/glob/grep | 8 | 7333 | 11068 | 3 |  | 2 | 2 |
| `packages/shell` | Оболочки: seam, bash/pwsh локально и в sandbox, инструменты bash/pwsh | 10 | 4894 | 11528 | 1 |  |  |  |
| `packages/subprocess` | Запуск процессов и деревьев процессов, Win32-хелперы | 3 | 6922 | 10376 | 1 |  |  | 1 |
| `packages/terminal` | Постоянные PTY-сессии | 2 | 2063 | 4025 |  |  |  | 1 |
| `packages/sandbox` | Ограничение процессов: политика, bwrap/Landlock/Seatbelt, Windows ACL | 4 | 3998 | 7431 | 2 |  |  | 2 |
| `packages/interaction` | Человек в контуре: approval, permission presets, вопросы пользователю, slash-команды | 5 | 3031 | 4331 | 4 |  | 1 |  |
| `packages/guard` | Гигиена цикла: напоминания о повторах, дедлайн tools/execute | 2 | 315 | 667 | 2 |  |  |  |
| `packages/compaction` | Сжатие контекста: summary, обрезка tool results, вынос изображений, /compact | 5 | 2911 | 6478 | 4 |  | 3 | 2 |
| `packages/context` | Контекст запроса: AGENTS.md, время, ссылки на файлы и сессии | 7 | 4582 | 10373 | 1 | 3 | 2 |  |
| `packages/skill` | Skills: реестр, файловый провайдер, инструмент `skill` | 5 | 2947 | 5589 | 3 | 1 | 2 |  |
| `packages/preset` | Agent presets: декларативный состав плагинов на сессию, persona | 5 | 1211 | 1327 |  | 1 | 2 |  |
| `packages/plan` | Plan mode | 1 | 580 | 1737 | 1 |  | 1 |  |
| `packages/todo` | Инструмент `todo_write` | 1 | 271 | 683 | 1 |  | 1 |  |
| `packages/goal` | Долгая цель сессии и её раунды | 4 | 2531 | 3596 | 4 |  | 2 |  |
| `packages/schedule` | Расписания (RFC 5545) и их инструменты | 2 | 4080 | 8010 |  | 1 | 1 |  |
| `packages/jobs` | Фоновые задания и инструменты job_* | 3 | 2462 | 4434 | 2 |  | 1 | 1 |
| `packages/subagent` | Субагенты: seam, in-process spawn/fork, ACP, Codex, Claude Code, dsh SDK, инструменты | 9 | 10036 | 27096 | 5 | 1 | 2 |  |
| `packages/team` | Agent Teams: общая доска задач | 2 | 1359 | 2926 | 2 |  |  |  |
| `packages/workflow` | Сценарии оркестрации субагентов (PTC) | 3 | 2392 | 3294 | 2 |  | 2 |  |
| `packages/ptc-runtime` | Programmatic tool calling: изолированный Node-рантайм для `run_code` | 2 | 4456 | 7781 | 1 |  |  |  |
| `packages/mcp` | MCP: клиент, менеджер, ресурсы | 3 | 3050 | 6695 | 1 | 1 |  | 1 |
| `packages/lsp` | LSP: seam, stdio-провайдер, инструмент | 3 | 2515 | 3443 |  |  |  |  |
| `packages/web` | Веб-поиск и fetch: seam, провайдеры, инструменты | 5 | 3898 | 6366 | 5 |  | 1 |  |
| `packages/document` | Office: рендер/конвертация, инструменты | 2 | 2160 | 3129 |  | 1 | 1 |  |
| `packages/deliverables` | Результаты хода: present, изменения в workspace | 3 | 1451 | 1801 |  | 2 | 1 |  |
| `packages/attachment` | Вложения: идентичность, локальное контентно-адресуемое хранение | 2 | 2269 | 2286 | 1 |  |  | 1 |
| `packages/spill` | Вынос больших tool results в файлы | 3 | 1211 | 1959 | 2 |  |  |  |
| `packages/ssh` | Удалённое исполнение по SSH (fs/subprocess/sandbox) | 5 | 2058 | 4232 |  |  |  |  |
| `packages/credentials` | Учётные данные, авторизация, аккаунт DeepSeek | 4 | 3410 | 5387 | 3 |  |  |  |
| `packages/settings` | Пользовательские настройки (файл) | 1 | 704 | 800 |  |  |  |  |
| `packages/identity` | Анонимный идентификатор | 1 | 100 | 105 |  |  |  |  |
| `packages/telemetry` | OTel-каналы | 1 | 475 | 466 | 1 |  |  |  |
| `packages/feedback` | Обратная связь по сообщениям | 2 | 687 | 1449 | 1 | 1 |  |  |
| `packages/boot` | Загрузка: app-boot (профили, слои патчей), hmr, plugin-manager, plugin-market, config-editor, cmdline | 6 | 13497 | 22436 |  |  |  |  |
| `packages/bundle` | Bundles: base, web-app, headless, sdk-app, sdk-minimal, acp-app | 6 | 1548 | 4073 |  | 1 |  | 1 |
| `packages/extensions` | Самоинспекция Cordis для агента, runner динамических пакетов (включая сгенерированные каталоги API) | 4 | 27507 | 5154 |  | 4 |  |  |
| `packages/sdk` | Внепроцессный SDK: JSON-RPC по stdio (протокол, сервер, TS-клиент) | 3 | 3046 | 7104 |  |  |  | 1 |
| `packages/acp` | Сервер Agent Client Protocol | 1 | 1782 | 3583 |  |  |  |  |
| `packages/api` | Remote-слой Host↔Client: Typert RPC gateway, контроллеры | 9 | 24583 | 47090 | 2 | 7 |  |  |
| `packages/typert` | Typert: генерация типов/схем для RPC, реестр | 11 | 11333 | 9794 | 2 |  |  |  |
| `packages/host` | Серверная часть Web GUI: webserver, auth, выбор каталогов, инвентарь плагинов | 11 | 5277 | 6615 |  | 5 |  |  |
| `packages/client` | Браузерная часть Web GUI: shell, connection, slots, ui-* плагины (React) | 74 | 217763 | 218092 |  | 62 |  |  |
| `packages/workspace` | Сущность Workspace | 1 | 1448 | 1587 |  | 1 |  |  |
| `packages/browser-use` | Регистрация browser-провайдера | 1 | 65 | 51 |  |  |  |  |
| `packages/computer-use` | Регистрация desktop-провайдера | 1 | 65 | 51 |  |  |  |  |
| `packages/experimental` | Экспериментальное: forge, CUA, hooks Claude Code/Codex, recommendations, liveshare, webhook и др. | 65 | 142460 | 154968 |  |  |  |  |
| `packages/test-support` | Тестовая инфраструктура: testkits, llm-replay, llm-mock-server, session-snapshot | 8 | 11824 | 12106 |  |  |  |  |
| `packages/util` | Утилиты: brand, error, пути, ripgrep, таймауты и др. | 24 | 8376 | 7629 |  |  |  |  |
| `apps` | Приложения: cli (`dsh`), web, desktop (Electron), desktop-host, publisher, генератор индекса плагинов | 12 | 30190 | 128029 |  |  |  |  |
| `native` | Нативные аддоны (launcher sandbox и системные вызовы) | 6 | 1772 | 987 |  |  |  |  |
| `python` | Python SDK и runtime-wheel | 1 | 558 | 0 |  |  |  |  |
| `website` | Сайт документации | 1 | 2473 | 1506 |  |  |  |  |
| **Итого** | | **420** | **662955** | **908190** | 87 | 96 | 25 | 33 |

Разрез по тому, что это за код:

| Часть | Пакетов | Строк src | Строк тестов | Доля src |
|---|---:|---:|---:|---:|
| Ядро Cordis и его спутники (`vendor`) | 9 | 7 484 | 0 | 1 % |
| Рантайм агента без интерфейса (остальные группы `packages/*`) | 209 | 179 731 | 323 849 | 27 % |
| Web UI и его транспорт (`client`, `api`, `typert`, `host`) | 105 | 258 956 | 281 591 | 39 % |
| Экспериментальное (`packages/experimental`) | 65 | 142 460 | 154 968 | 21 % |
| Самоинспекция и сгенерированные каталоги (`packages/extensions`) | 4 | 27 507 | 5 154 | 4 % |
| Приложения (`apps`: cli, web, desktop) | 12 | 30 190 | 128 029 | 5 % |
| Тестовая инфраструктура (`packages/test-support`) | 8 | 11 824 | 12 106 | 2 % |
| Прочее (`native`, `python`, `website`) | 8 | 4 803 | 2 493 | 1 % |
| **Итого** | **420** | **662 955** | **908 190** | 100 % |

Состав preset `standard` (25 безусловных пакетов, около 18,6 тыс. строк; строки с условием помечены): `persona`, `agent-instructions`, `time-context`, `tool-bash` либо `tool-pwsh` (условие по платформе), `tool-fs` (с `applyPatch: false`), `tool-fs-search`, `tool-jobs`, `tool-schedule`, `skill-filesystem`, `tool-skill`, `command-goal`, `tool-goal`, группа plan mode (`plan-mode` в собственном realm), группа compaction (`compaction-basic`, `command-compact`, `compaction-tool-result-pruner`), группа delegation (`tool-subagent-control`, `tool-subagent` с провайдерами spawn и fork, `workflow-ptc`, `tool-workflow`), `tool-ask-user`, `tool-todo`, `tool-web`, `tool-present`, `tool-office`, `tool-plugin-manager` (условно), `tool-plugin-market` (выключен). Preset не содержит позвоночник агента: тот приходит из `dsh-base`.

### 7.2. Цикл агента по шагам

Исходники: `packages/core/agent-loop/src/agent.ts` (751 строка, класс `ReactLoopAgent`), `index.ts` (838, плагин и конфигурация), `inbox.ts` (232), `assistant-stream.ts` (141), `runtime-context.ts` (181); интерфейс `Agent` и события в `packages/core/agent/src/`; конвейер инструментов в `packages/core/tools/src/pipeline.ts` (807) и `batch.ts` (285). Документы: `docs/architecture.md`, `docs/agent-lifecycle.md`, `docs/tool-execution-pipeline.md` (сгенерированный граф).

1. **Вход.** `send(message, target, wakeup)` (`agent.ts:171`) кладёт сообщение в inbox с целью `next-turn` или `next-step`. Три обёртки: `followup` (следующий ход, будит драйвер), `steer` (следующий шаг текущего хода, будит), `inject` (следующий шаг, не будит; так плагины добавляют контекст). Inbox устойчив: каждая вставка и изъятие записываются событием `agent/inbox/spliced`. `cancel(cause)` (`agent.ts:192`) очищает inbox и прерывает текущую фазу через `AbortController`; причина одна из `user`, `parent`, `hook`, `disposed`.
2. **Драйвер.** `kick()` (`agent.ts:274`) крутит `while (await this.turn()) {}`. По выходе фаза становится `idle`; если за это время пришёл будящий ввод, драйвер запускается снова. Статус агента (`idle` или `running`) объявляется событием `agent/status`.
3. **Начало хода.** `turn()` (`agent.ts:333`) пишет `turn/start`.
4. **Подготовка шага.** `preStep()` (`agent.ts:289`): изъять сообщения из inbox; взять текущий выбор модели; `ctx.tools.resolve(...)` собирает набор инструментов под агента и модель; `ctx.systemPrompt.assemble(...)` собирает system prompt и схемы; затем waterfall `agent/pre-step`, где плагины могут отклонить шаг или изменить входные сообщения. По умолчанию к сообщениям добавляется снимок runtime context (текущая политика файлов и подтверждений, рабочий каталог) как `user/message` с источником-плагином. Compaction подключается здесь и на ошибке запроса.
5. **Запрос.** `step()` (`agent.ts:441`) пишет `step/start`; `prepareRequest()` и `buildRequest()` (`agent.ts:619`, `662`) проводят запрос через `agent/request`. В журнал попадают `system/message`, `user/message`, а также `request/header` (system prompt и схемы инструментов, под которыми построен запрос). Заголовок пишется только при изменении: сравнение `headerEquals` в `packages/core/session/src/request-header.ts`. Это держит префикс запроса стабильным для кэша промпта.
6. **Поток ответа.** Адаптер отдаёт `StreamChunk`; живые фрагменты уходят наблюдателям событием `agent/assistant-stream` и в журнал не пишутся; итог пишется `assistant/message`, неудачная попытка `assistant/attempt`. Ошибка проходит через `agent/request-error`: там решают о повторе `llm-retry` и compaction (при переполнении контекста сжать и повторить).
7. **Инструменты.** Если ответ содержит вызовы, выбранный исполнитель (`tool-executor-native`, либо `tool-executor-ptc` для режима `run_code`) передаёт пакет в `ctx.tools.execute`. `batch.ts` планирует: параллельно-безопасные вызовы идут ограниченным пулом (`maxParallelToolCalls`), остальные исключительным барьером. Каждый вызов проходит конвейер:

   ```text
   tool/call (в журнал до исполнения)
     -> waterfall tools/pre-execute   : allow | deny | ask (ask -> ctx.approval, отказ по умолчанию)
     -> монотонные guards             : только запретить или воздержаться
     -> waterfall tools/execute       : таймаут, повтор, метрики вокруг тела
     -> execute() инструмента         : для tool-fs внутри ещё fs/write-intent, fs/edit-intent
     -> projectContent
     -> waterfall tools/post-execute  : принять, заблокировать, заменить, добавить контекст
     -> finalizeContent
     -> tools/result (синхронное уведомление)
   tool/result (в журнал, единственный исход, видимый модели)
   ```

   Исключение на любом этапе превращается в результат с `isError`, а не роняет цикл.
8. **Конец шага и хода.** `step/end`. Если были вызовы инструментов или появился ввод `next-step`, идёт следующий шаг. Иначе waterfall `agent/turn-stopping` даёт плагинам шанс вмешаться перед остановкой (его слушают `deliverables/workspace-changes` и экспериментальные `hooks-claude-code`, `hooks-codex`; цели продолжают работу через `agent/pre-step`, пакет `goal/goal-round-driver`). Затем `turn/end` с причиной из закрытого набора: `completed`, `aborted`, `blocked`, `error`, `max-tokens`, `interrupted`, `forked` (`packages/core/session/src/types.ts`, `TurnEndReasonMap`).
9. **Восстановление.** Состояние агента реконструируется из журнала. `packages/core/session/src/repair.ts` закрывает оборванный хвост синтетическими событиями: вызов, который не начался, получает код `TOOL_NOT_STARTED`, начатый без записанного исхода получает `TOOL_OUTCOME_UNKNOWN`.

Что из этого важно сохранить в Rust: цикл сам ничего не знает о разрешениях, sandbox, compaction, retry и целях. Всё это слушатели пяти точек: `agent/pre-step`, `agent/request`, `agent/request-error`, `agent/turn-stopping` и трёх waterfall конвейера инструментов. Размер подтверждает: сам цикл 2,2 тыс. строк, а его тесты 15,4 тыс.

### 7.3. Встроенные инструменты и их схемы

Каталог `docs/tool-catalog.md` сгенерирован из кода и содержит 77 инструментов; полный список с параметрами и исходными файлами дан в приложении B. Ниже те, что нужны для агента общего назначения.

| Инструмент | Пакет | Параметры (`*` — обязательный) | Исходник |
|---|---|---|---|
| `bash` | `dsh-tool-bash` | `description`*: string; `command`*: string; `timeoutMs`: number; `workdir`: string; `run_in_background`: boolean | `packages/shell/tool-bash/src/index.ts` |
| `read` | `dsh-tool-fs` | `file_path`*: string; `offset`: number; `limit`: number | `packages/fs/tool-fs/src/index.ts` |
| `write` | `dsh-tool-fs` | `file_path`*: string; `content`*: string | `packages/fs/tool-fs/src/index.ts` |
| `edit` | `dsh-tool-fs` | `file_path`*: string; `old_string`*: string; `new_string`*: string; `replace_all`: boolean | `packages/fs/tool-fs/src/index.ts` |
| `apply_patch` | `dsh-tool-fs` | `input`*: string | `packages/fs/tool-fs/src/index.ts` |
| `read_image` | `dsh-tool-fs` | `file_path`*: string | `packages/fs/tool-fs/src/index.ts` |
| `glob` | `dsh-tool-fs-search` | `pattern`*: string; `path`: string | `packages/fs/tool-fs-search/src/index.ts` |
| `grep` | `dsh-tool-fs-search` | `pattern`*: string; `path`: string; `include`: string | `packages/fs/tool-fs-search/src/index.ts` |
| `working_directory` | `dsh-tool-working-directory` | `cd`: string | `packages/session/tool-working-directory/src/index.ts` |
| `job_output` | `dsh-tool-jobs` | `job_id`*: string; `wait`: boolean; `timeout_ms`: number | `packages/jobs/tool-jobs/src/index.ts` |
| `job_kill` | `dsh-tool-jobs` | `job_id`*: string; `reason`: string | `packages/jobs/tool-jobs/src/index.ts` |
| `job_list` | `dsh-tool-jobs` | (нет параметров) | `packages/jobs/tool-jobs/src/index.ts` |
| `todo_write` | `dsh-tool-todo` | `todos`*: array[object] | `packages/todo/tool-todo/src/index.ts` |
| `ask_user_question` | `dsh-tool-ask-user` | `questions`*: array[object] | `packages/interaction/tool-ask-user/src/index.ts` |
| `exit_plan_mode` | `dsh-plan-mode` | `plan`*: string | `packages/plan/plan-mode/src/index.ts` |
| `skill` | `dsh-tool-skill` | `name`*: string | `packages/skill/tool-skill/src/index.ts` |
| `subagent` | `dsh-tool-subagent` | `cwd`: string; `description`*: string; `name`: string; `prompt`*: string | `packages/subagent/tool-subagent/src/index.ts` |
| `list_subagent_models` | `dsh-tool-subagent` | `provider`: string; `model`: string | `packages/subagent/tool-subagent/src/list-models.ts` |
| `send_message` | `dsh-tool-subagent-control` | `agent`*: string; `message`*: string | `packages/subagent/tool-subagent-control/src/index.ts` |
| `wait_agent` | `dsh-tool-subagent-control` | `timeout_ms`: integer | `packages/subagent/tool-subagent-control/src/wait-agent.ts` |
| `list_agents` | `dsh-tool-subagent-control` | (нет параметров) | `packages/subagent/tool-subagent-control/src/list-agents.ts` |
| `interrupt_agent` | `dsh-tool-subagent-control` | `agent`*: string | `packages/subagent/tool-subagent-control/src/index.ts` |
| `web_fetch` | `dsh-tool-web` | `url`*: string | `packages/web/tool-web/src/index.ts` |
| `web_search` | `dsh-tool-web` | `queries`*: array[string] | `packages/web/tool-web/src/index.ts` |
| `run_code` | `dsh-tool-executor-ptc` | `code`*: string; `description`*: string; `timeoutMs`: number; `sandbox_permissions`: enum(workspace-write, danger-full-access); `justification`: string | `packages/core/tool-executor-ptc/src/ptc.ts` |
| `plugin_manager` | `dsh-plugin-manager` | `action`*: string; `target`: string; `enabled`: boolean; `runtimeVersion`: string; `acceptRisk`: boolean; `approvedBuilds`: array[string]; `registry`: string; `offset`: number; `limit`: number | `packages/boot/plugin-manager/src/tools.ts` |

Определение инструмента (`packages/core/tools/src/contracts.ts`, `ToolDefinition`; пример из `docs/cordis-tutorial/07-into-the-harness.md`):

```ts
ctx.tools.register(defineTool({
  name: 'greet',
  description: 'Greet the named person.',
  parameters: {
    name: { type: 'string', required: true, description: 'Who to greet' },
  },
  output: {
    schema: { type: 'string' },
    render: (_args, value) => [{ type: 'text', text: value }],
  },
  async execute(args) {
    return `Hello, ${args.name}!`
  },
}))
```

Особенности, которые придётся повторить или сознательно упростить:

- Аргументы вызова идут сквозь всю систему как **сырая строка JSON** (`arguments`), разбор делает конвейер. Это контракт адаптера (7.4).
- У инструмента есть схема выхода и отдельные проекции для модели и для интерфейса (`presentCall`, `presentResult`, `projectContent`, `finalizeContent`).
- Режим исполнения (параллельный или исключительный) определяется для каждого вызова, а не для инструмента целиком (`pipeline.executionMode`).
- `ToolSchema.deferLoading` позволяет не отдавать модели схему сразу (отложенные инструменты; в снимках есть сценарии `deepseek-deferred-tools`).
- В минимальном профиле набор другой: постоянная оболочка (`bash` из `dsh-tool-bash-persistent`), `read_image` и `working_directory`; текстовых файловых инструментов нет (`packages/bundle/sdk-minimal/README.md`).

### 7.4. Интерфейс адаптера модели и формы API

Seam: `packages/llm/llm/src` (4,2 тыс. строк). Адаптер есть класс с одним обязательным методом (`index.ts:223–317`):

```ts
abstract class LlmAdapter {
  abstract stream(options: GenerateOptions): AsyncIterable<StreamChunk>
  listModels(provider: string): Promise<readonly LlmModelInfo[]>   // необязательно
  resolveModel(...)                                                // метаданные модели: context, reasoning
  prepareCall(provider, model, signal?): Promise<PreparedAdapterCall>
}
// регистрация как эффект:
ctx.llm.registerAdapter(['my-provider'], new MyAdapter(...))
```

Запрос и поток (`packages/llm/llm/src/types.ts`):

```ts
interface GenerateOptions {
  provider: string; model: string; reasoningEffort?: ReasoningEffortId
  messages: RequestMessage[]; system?: string
  tools?: ToolSchema[]; toolHistory?: ToolHistory
  temperature?: number; maxTokens?: number; stop?: string[]
  signal?: AbortSignal; sessionId?: SessionId; purpose?: Purpose
}
type StreamChunk =
  | { type: 'block-start'; index: number; blockType: ContentBlockType }
  | { type: 'text-delta'; index: number; text: string }
  | { type: 'reasoning-delta'; index: number; text: string }
  | { type: 'tool-call-delta'; index: number; id: ToolCallId; name?: string; argumentsDelta: string }
  | { type: 'block-end'; index: number; block: ContentBlock }
  | { type: 'usage'; usage: TokenUsage }
  | { type: 'finish'; reason: FinishReason; replayState?: ReplayEnvelope }
// FinishReason.kind: 'stop' | 'tool-calls' | 'max-tokens' | 'aborted' | 'error'
```

Обязательства адаптера (`docs/cookbook/adding-an-llm-adapter.md`), подтверждённые двумя реализациями:

- `usage` идёт до `finish`, после `finish` ничего;
- аргументы инструментов остаются сырой строкой JSON и передаются фрагментами `argumentsDelta`;
- индексы блоков выдаются в порядке первого появления;
- два пути ошибки: исключение `LlmError` со стабильным кодом для транспорта и протокола либо `finish` с `error` или `aborted` для отказов провайдера внутри потока;
- `signal` обязан прерывать запрос;
- параметр, который провайдер не поддерживает, вызывает `LlmError(..., 'UNSUPPORTED_OPTION')`, а не молча отбрасывается;
- если провайдеру для продолжения нужны собственные метаданные (идентификаторы ответов, подписи рассуждений), адаптер отдаёт их в `finish.replayState`, и рантайм возвращает их только тому же экземпляру адаптера.

Реализаций две. `llm-deepseek` (3,7 тыс. строк) говорит по HTTP напрямую в формате DeepSeek Messages, SSE разбирается `eventsource-parser`; cookbook называет его образцовой раскладкой (типы провода, сериализация запроса, разбор транспорта, перевод в чанки, класс адаптера). `llm-pi-ai` (4,8 тыс. строк) оборачивает библиотеку `@earendil-works/pi-ai` 1.3.0 с локальным патчем и поддерживает четыре протокола (`packages/llm/llm-pi-ai/src/provider.ts`):

| `api` | Протокол |
|---|---|
| `openai-completions` | OpenAI Chat Completions |
| `openai-responses` | OpenAI Responses |
| `anthropic-messages` | Anthropic Messages |
| `google-generative-ai` | Gemini |

Локальная модель за OpenAI-совместимым сервером подключается без кода (`docs/user/guide/providers.md`):

```yaml
- id: llm-pi-ai
  config:
    providers:
      my-gateway:
        apiKeyEnv: GATEWAY_API_KEY
        api: openai-completions
        baseURL: https://gateway.example/v1
        compat:
          supportsDeveloperRole: false
          maxTokensField: max_tokens
        models:
          - id: my-model
```

Обнаружение моделей идёт через `GET /models`. Главный урок этого адаптера для нас: «OpenAI-совместимый» сервер совместим не полностью, и расхождения закрываются длинным списком переключателей `compat` (`PiAiCompatProfile` в `docs/config-catalog.md`): `supportsDeveloperRole`, `maxTokensField`, `supportsUsageInStreaming`, `supportsFinishReason`, `requiresToolResultName`, `requiresAssistantAfterToolResult`, `requiresThinkingAsText`, `requiresReasoningContentOnAssistantMessages`, `supportsStrictMode`, `supportsTemperature`, `thinkingFormat`, `chatTemplateKwargs`, `thinkingTokenBudgetField`, `vllmPriority` и другие. Среди значений `thinkingFormat` есть `qwen` и `qwen-chat-template` (`packages/llm/llm-pi-ai/src/catalog.ts`), а `chatTemplateKwargs` передаётся как `chat_template_kwargs`. То есть управление рассуждениями Qwen зависит от сервера и шаблона чата. Какой сервер стоит у заказчика и какой из двух форматов ему нужен, в этом исследовании не проверялось.

Для Rust: один адаптер Chat Completions со стримингом и таблицей `compat`, написанный напрямую, покрывает первую поставку. Контракт `StreamChunk` стоит взять без изменений: он не зависит от провайдера и уже проверен на двух реализациях.

### 7.5. Форматы сессий и хранения

**Журнал сессии** есть единственный источник истины: всё, что видит модель, должно восстанавливаться из него (`docs/architecture.md`). Формат версионирован: `SESSION_FORMAT_VERSION = 4` в `packages/core/session/src/types.ts`; учёт версий и статуса выпуска в `docs/session-format-status.md`; исторические схемы v0–v3 и цепочка соседних миграций в `docs/persistence-changes/` и `packages/session/session-format-catalog`. Проект считает формат обязательством даже в предрелизе: «GitHub's prerelease flag does not make persisted user data disposable».

Файл: первая строка заголовок, дальше по одному событию на строку (пример из `snapshots/session/fs-write-overwrite/session.v2.jsonl`; в фикстурах идентификаторы заменены токенами, а порядковые номера и время опущены):

```json
{"type":"session","version":2,"id":"{{session:1}}","createdAt":1783352092215,"cwd":"{{cwd}}","isSeeded":false,"delegationDepth":0}
{"type":"permission/preset","data":{"preset":"danger-full-access"}}
{"type":"sandbox/mode","data":{"mode":"danger-full-access"}}
{"type":"approval/policy","data":{"policy":"never"}}
{"type":"agent/inbox/spliced","data":{"target":"next-turn","start":0,"inserted":[{"content":[{"type":"text","text":"..."}],"source":{"kind":"user"},"role":"user","id":"{{message:1}}"}]}}
{"type":"turn/start","data":{"turn":1}}
{"type":"step/start","data":{"turn":1,"step":1}}
```

Словарь событий (`packages/core/session/src/known-event-types.ts`, около 60 типов), по смыслу:

| Группа | Типы |
|---|---|
| Ход и шаг | `turn/start`, `turn/end`, `step/start`, `step/end`, `agent/inbox/spliced` |
| Диалог | `system/message`, `developer/message`, `user/message`, `assistant/message`, `assistant/attempt`, `request/header`, `request/context` |
| Инструменты | `tool/call`, `tool/result`, `tool/ptc-dispatch`, `tool/ptc-dispatch-start`, `todo/write`, `job/registered`, `job/settled` |
| Политика | `permission/preset`, `sandbox/mode`, `approval/policy`, `approval/asked`, `approval/decided`, `plan/mode` |
| Модель | `model/selection`, `llm/retry`, `llm/retry-started` |
| Сжатие | `compaction/start`, `compaction/end`, `compaction/summary`, `compaction/prune`, `image/offload` |
| Субагенты и команды | `subagent/catalog`, `subagent/descriptor`, `subagent/model-selection-policy`, `team/*`, `tool-workflow/*` |
| Прочее | `session/title`, `session/end-seed`, `goal/change`, `schedule/change`, `working-directory/change`, `workspace/changes`, `command/run`, `command/done`, `hook/invoked`, `hook/result`, `feedback/*`, `deliverables/presented`, `agent-preset/selected` |

Плагины расширяют словарь слиянием деклараций `SessionEventMap`. В Rust это открытый набор: событие с неизвестным типом обязано читаться и сохраняться без потерь.

**Хранение журнала**: единственный собственный провайдер `session-persistence-jsonl` (5,2 тыс. строк, тесты 9,6 тыс.). Раскладка на диске (`packages/session/session-persistence-jsonl/README.md`):

```text
<root>/
  --<normalized-cwd>--/          # каталог проекта (или _no-cwd/)
    <encoded-id>/                # каталог сессии
      session.v4.jsonl.zstd      # по умолчанию: независимые кадры Zstandard с контрольной суммой
      session.v4.jsonl           # при compression: 'none'
      session.v3.jsonl[.zstd]    # сохранённые исторические поколения
```

Свойства: только дозапись; кадр на заголовок и кадр на каждую пачку записи; восстановление оборванного хвоста; миграция исторического поколения при открытии; `root` обязателен и не имеет значения по умолчанию; удаления сессий в интерфейсе нет. Минимальный профиль пишет несжатый JSONL.

**Производное состояние**: `session-projection` (синхронные проекции из событий: todo, цели, статистика; могут сохраняться контрольными точками), `token-meter` (оценка занятости контекста пересчётом журнала без обращения к модели).

**Несессионные данные**: `storage`, `storage-json`, `storage-sqlite`, `storage-domain` (записи workspace, sidecar-данные сессий); настройки пользователя в файле; учётные данные в `$DSH_HOME/.credentials.yaml`; вложения в контентно-адресуемом хранилище (`attachment-local`); большие результаты инструментов выносятся в файлы (`spill`). Домашний каталог: `$DSH_HOME/profiles/<name>/` (манифест профиля, `cordis.patch.yml`, установленные плагины, `compatibility.json`), `$DSH_HOME/cordis.patch.yml`, `$DSH_HOME/.env`.

### 7.6. Система разрешений

Три независимых механизма и один объединяющий переключатель.

| Механизм | Пакеты | Значения | Где применяется |
|---|---|---|---|
| Режим sandbox | `sandbox/sandbox`, `sandbox-local`, `sandbox-policy`, `sandbox-windows-acl`, `fs/fs-sandbox`, `shell/bash-sandbox` | `read-only`, `workspace-write`, `danger-full-access` | подпроцессы (bash, pwsh, терминал) и файловые инструменты; режим хранится в журнале (`sandbox/mode`) |
| Политика подтверждений | `interaction/user-approval` | `ask`, `never` | waterfall `tools/pre-execute` и запросы на расширение sandbox; хранится в журнале (`approval/policy`) |
| Guards | `guard/*`, произвольные плагины | запретить или воздержаться | монотонный этап конвейера: разрешить то, что запрещено ранее, guard не может |
| Preset | `interaction/permission-presets` | `workspace-write` = sandbox `workspace-write` + approval `ask`; `danger-full-access` = sandbox `danger-full-access` + approval `never`; несовпадающие сочетания показываются как `custom`; отдельная интеграция может добавить `auto` | выбор пользователя в интерфейсе; хранится в журнале (`permission/preset`) |

Подтверждение (`docs/subsystems/approval.md`): исход закрыт и отказоустойчив в сторону запрета: `allowed-once`, `rejected`, `cancelled`, `unavailable`. Разрешение действует на один вызов; постоянных «разрешить всегда» нет. Политика `never` отклоняет запрос до опроса отвечающих, поэтому плагин, подключённый позже, её не обойдёт. Отсутствующий или упавший отвечающий даёт `unavailable`, и действие не выполняется. Запрос и решение пишутся в журнал (`approval/asked`, `approval/decided`), в диалог модели они не попадают: модель видит только результат инструмента и текущий снимок политики. Запрос не дублирует аргументы инструмента, а ссылается на `callId` уже показанного вызова.

Sandbox (`packages/sandbox/README.md`): ограничиваются только файловые эффекты подпроцесса и его потомков. Бэкенды по платформам (`packages/sandbox/sandbox-local/README.md`): Linux (сначала `bwrap`, затем Landlock; статический launcher Landlock поставляется из `native/system` сборками `linux-x64` и `linux-arm64`), macOS (Seatbelt), Windows (runner с ограниченным токеном и ACL: capability SID и пониженный уровень целостности; гарантия названа частичной). Если режим не может быть обеспечен, вызов завершается `SANDBOX_UNAVAILABLE`, а не исполняется без ограничений. После отказа модель может запросить один более широкий режим, и это требует подтверждения человека. Сеть и видимость процессов вне гарантий. Для полной изоляции документация отсылает к контейнерам, microVM и удалённым исполнителям, которые подменяют провайдеров `fs` и `subprocess` целиком.

Plan mode разрешений не ограничивает: «every tool remains available, so use sandbox mode and approval prompts for enforced limits» (`packages/plan/plan-mode/README.md`).

Для Rust: модель из двух ручек и закрытого исхода мала и переносится целиком. Трудоёмкая часть это платформенные бэкенды sandbox; для первой поставки нужен только Linux aarch64.

### 7.7. Web UI и транспорт к ядру

Клиент это отдельное приложение Cordis в браузере, собранное из лениво загружаемых плагинов (`docs/subsystems/web-client.md`): 74 пакета, около 218 тыс. строк исходников и столько же тестов.

| Слой | Чем сделан | Источник |
|---|---|---|
| Интерфейс | React 18 (`react`, `react-dom` `^18.2.0`), `@tanstack/react-virtual`; Markdown на micromark и mdast, подсветка shiki, формулы katex; `lexical` 0.49.0 с локальным патчем (зависимость `ui-conversation`) | `apps/web/package.json`, `packages/client/ui-primitives/package.json`, `pnpm-workspace.yaml` |
| Сборка | Vite, `@vitejs/plugin-react`; библиотеки собираются tsdown | `apps/web/package.json`, `package.json` |
| Расширяемость | система слотов (`ctx.slots.register`), хранилища, клиентские плагины; пакет плагина объявляет `dsh.client`, хост отдаёт его бандл по `/plugins`, браузер грузит лениво | `packages/client/AGENTS.md`, `packages/client/modules/README.md` |
| Вызовы | Typert: генератор времени сборки превращает типы TypeScript хост-сервисов в дескрипторы, кодеки и схемы; клиент вызывает `ctx.remote.<namespace>.<method>` | `packages/typert/README.md`, `packages/api/gateway/README.md` |
| Транспорт | унарные вызовы: HTTP POST на `/api`; потоки: WebSocket `/api/remote.mux` с мультиплексированием логических потоков, у каждого есть обратный канал от клиента; heartbeat; пересоздание «поколения» соединения при обрыве | `packages/client/connection/README.md`, `packages/api/gateway/README.md` |
| Сервер | `node:http` (или `node:https`), таблица маршрутов, которую наполняют плагины; сам сервер файлов не отдаёт | `packages/host/webserver/README.md` |
| Доступ | каждый вызов и поток требуют аутентифицированного Peer: токен в подписанной ссылке входа, обмен на cookie, проверка заголовка `Host` (loopback или список доверенных) | `packages/client/connection/README.md` |
| Desktop | Electron (в `pnpm-workspace.yaml` закреплён 44.7.0); страница грузится по `file://`, вызовы идут через IPC-мост | `apps/desktop/README.md`, `packages/host/webserver/README.md` |

Кроме Web есть два внепроцессных входа, и для Rust они важнее:

- **SDK**: JSON-RPC 2.0, по одному сообщению на строку через stdio (`packages/sdk/protocol/src/types.ts`). Методы: `initialize`, `session/prompt`, `session/query`, `session/resume`, `session/wait`, `session/working-directory/get`, `session/working-directory/set`, `shutdown`; запрос от сервера к клиенту `approval/request`; уведомления о событиях сессии, смене статуса, старте и завершении субагента. Тот же протокол использует Python SDK.
- **ACP**: сервер Agent Client Protocol (`packages/acp`), профиль `acp`.

Вывод: переносить клиент нецелесообразно. Он один больше всего рантайма без интерфейса (разрез в 7.1), а его транспорт привязан к генератору из типов TypeScript. Переиспользовать готовый клиент поверх ядра на Rust теоретически можно (лицензия MIT), но тогда ядро обязано воспроизвести провод Typert и раздачу клиентских бандлов; эта совместимость сломается при первом же изменении upstream. Практичный путь: собственный интерфейс поверх потока событий сессии, а внешним контрактом сделать протокол уровня SDK.

### 7.8. Тесты, пригодные как проверка соответствия

Политика и ярусы описаны в `docs/testing.md`.

| Набор | Что это | Пригодность для Rust |
|---|---|---|
| Тесты ядра upstream: `[cordis] packages/core/tests` (17 spec-файлов, около 4,3 тыс. строк: `fiber`, `dispose`, `service-dispose`, `isolate`, `intercept`, `events`, `update`, `replace`, `shadow`, `associate` и др.) | поведение жизненного цикла, зависимостей, областей, событий | Высокая. Это спецификация семантики ядра в виде сценариев; переписываются на Rust один к одному |
| `[dsh] scripts/cordis-*.spec.ts`, `loader-*.spec.ts`, `volatile-*.spec.ts` (14 файлов, около 2 тыс. строк) | проверки вендоренного ядра и loader: жизненный цикл, записи YAML, журнал include, diff конфигурации | Высокая, тем же способом |
| `snapshots/session` (194 сценария) | на сценарий: `snapshot.yml` (профиль, политика записи), журналы `session.vN.jsonl`, исходный `workspace/` и ожидаемый `workspace.expected/`. Тест берёт из журнала ввод пользователя и поток модели (пакет `llm-replay`), прогоняет настоящий агент без ключа и сравнивает получившийся журнал и дерево файлов | Средняя и самая ценная. Данные не зависят от языка. Но сравнение журнала требует совпадения текста system prompt, runtime context, имён и схем инструментов. Без совместимости «буква в букву» из сценария остаются: ввод, поток модели, последовательность типов событий и `workspace.expected/` |
| `snapshots/sdk` (41 сценарий), `snapshots/acp` (9) | то же через процесс `dsh` и протоколы SDK и ACP | Высокая, если наш бинарник говорит тем же JSON-RPC: проверка идёт снаружи процесса |
| `snapshots/web` (77 сценариев) и `apps/web/tests/expected` | браузерные снимки (Chromium, ARIA) | Нет, привязаны к клиенту |
| `packages/core/agent-loop/tests` (28 spec-файлов, 15,4 тыс. строк; среди них `contract-regressions.spec.ts`, `properties.spec.ts`, `resume.spec.ts`, `cancel.spec.ts`, `tool-order.spec.ts`) | модульные тесты цикла против внутрипроцессного API | Напрямую не запускаются. Используются как перечень контрактов и регрессий, которые нужно повторить |
| `packages/test-support/llm-mock-server` | программируемый HTTP/SSE-сервер в формате Messages (`/v1/messages`): обрывы, зависания, битые чанки | Средняя. Идея переносится; сам сервер говорит не Chat Completions |
| `benchmarks/` (`session-open`, `agent-continuation`, `conversation-fold`, `terminal-io` и др.) | бюджеты времени и памяти на синтетике | Низкая: пороги привязаны к Node |
| Real-API e2e (`*.e2e.ts`) | живые провайдеры, с ключом | Нет |

Оговорки. Покрытие 100 % по файлам в dsh обязательно, и разбор 0001 показывает его предел: продукт падал на первом запросе при зелёных тестах, потому что тесты монтировали плагин не тем путём, каким он грузится в поставке. Отсюда правило самого проекта «Test the real entry path». Для нас это означает: проверка соответствия должна идти через собранный бинарник и настоящий профиль. Ни один из наборов в этом исследовании не запускался.

### 7.9. Минимальный набор плагинов

Точка отсчёта у dsh: профиль `sdk-minimal` (37 строк конфигурации, 33 безусловных пакета, около 43,7 тыс. строк исходников и 88 тыс. строк тестов). В нём есть цикл, модель, журнал, постоянная оболочка, `read_image`, `working_directory`, compaction и сервер SDK. По `packages/bundle/sdk-minimal/README.md` исключены `dsh-base`, Web, настройки, учётные данные, телеметрия, текстовые файловые инструменты, инструкции проекта, skills, jobs и субагенты (строка `dsh-jobs-local` в патче при этом есть, инструментов `job_*` нет). Подтверждений тоже нет: профиль работает с политикой `danger-full-access` и рассчитан на изолированный workspace.

Для первой поставки этого мало: пользователям базового уровня нужны подтверждения и sandbox, а продукту нужны чужие harness и задачи. Предлагаемый состав, с размером образца в dsh:

| Слой | Образец в dsh (пакеты) | Строк src | Строк тестов |
|---|---|---:|---:|
| Ядро (не плагины): контекст, fiber, эффекты, сервисы, события, loader, слои конфигурации, схемы настроек | `vendor/cordis`, `vendor/loader`, `vendor/include`, `vendor/timer`, `vendor/schemastery`, `boot/app-boot`, `boot/config-editor` | 11 871 | 9 882 |
| Позвоночник агента | `core/session`, `core/system-prompt`, `core/tools`, `core/agent`, `core/agent-loop`, `core/scope`, `core/tool-executor-native` | 12 500 | 31 057 |
| Модель: seam, повторы, счётчик токенов, один адаптер (образец раскладки: `llm-deepseek`) | `llm/llm`, `llm/llm-retry`, `llm/token-meter`, `llm/llm-deepseek` | 9 941 | 17 579 |
| Журнал и производное состояние | `session/session-persistence-jsonl`, `session/session-projection`, `session/session-title`, `session/working-directory`, `session/tool-working-directory`, `attachment/attachment-local` | 9 123 | 15 628 |
| Исполнение: процессы, файлы, оболочка, фоновые задания | `subprocess/subprocess-local`, `fs/fs`, `fs/fs-local`, `fs/tool-fs`, `fs/tool-fs-search`, `shell/shell`, `shell/bash-local`, `shell/tool-bash`, `jobs/jobs-local`, `jobs/tool-jobs` | 14 555 | 25 585 |
| Разрешения и sandbox | `sandbox/sandbox`, `sandbox/sandbox-local`, `sandbox/sandbox-policy`, `fs/fs-sandbox`, `shell/bash-sandbox`, `interaction/user-approval`, `interaction/permission-presets`, `interaction/tool-ask-user` | 3 584 | 6 822 |
| Контекст: сжатие, инструкции проекта, время, preset | `compaction/compaction-basic`, `compaction/compaction-tool-result-pruner`, `compaction/command-compact`, `context/agent-instructions`, `context/time-context`, `preset/agent-preset`, `preset/persona` | 4 812 | 13 321 |
| Задачи: skills, todo, plan mode | `skill/skill`, `skill/skill-filesystem`, `skill/tool-skill`, `todo/tool-todo`, `plan/plan-mode` | 3 269 | 6 465 |
| Чужие harness и внешние инструменты: субагенты, ACP, Claude Code, Codex, MCP | `subagent/subagent`, `subagent/subagent-acp`, `subagent/subagent-claude-code`, `subagent/subagent-codex`, `subagent/tool-subagent`, `subagent/tool-subagent-control`, `mcp/mcp-client` | 11 508 | 29 904 |
| Внешний контракт: протокол и сервер SDK | `sdk/protocol`, `sdk/server` | 1 620 | 4 970 |
| **Итого, пакетов: 63** | | **82 783** | **161 213** |

Образец этого состава занимает около 83 тыс. строк исходников (12 % репозитория) и около 161 тыс. строк тестов. Это размер образца на TypeScript, а не оценка объёма на Rust: часть кода обслуживает особенности Node и истории проекта (деревья процессов на трёх ОС, миграции исторических форматов журнала v0–v3, проверки совместимости npm-пакетов).

Что в этот состав сознательно не входит: `dsh-hmr` с заменой кода, Plugin Manager поверх npm и pnpm, plugin market, Typert и весь клиент, `tool-executor-ptc` и `ptc-runtime` (исполнение программ модели в Node), workflow, schedule, goal, agent teams, office, LSP, SSH, телеметрия, учётные записи DeepSeek, всё из `packages/experimental`. Каждое из этого есть отдельный плагин и добавляется позже без изменения ядра; это и есть проверка того, что архитектура перенесена верно.

Что придётся написать без образца в dsh: адаптер Chat Completions напрямую (в dsh это внешняя библиотека pi-ai), провайдеры для OpenCode и Qwen Code, возобновляемое и наблюдаемое управление чужим harness (в dsh провайдеры одноразовые, раздел 3.6), task-плагины с памятью и источниками (раздел 4), собственный интерфейс.

## 8. Что из найденного должно изменить наш дизайн

1. **Ядро небольшое, продукт огромный.** Семантика Cordis умещается примерно в 5 тыс. строк (ядро, loader, include). Рантайм агента без интерфейса около 180 тыс. строк, клиент ещё 218 тыс. Переписывание «по образу» реалистично только как ядро плюс тонкий состав плагинов (раздел 7.9), а не как паритет.
2. **Переносить семантику, а не механику JavaScript.** Нужны: эффект с обратным действием, владение эффектами по fiber, committed view, ожидание зависимых при выгрузке провайдера, реактивный `inject`, слои конфигурации. Не нужны: `Proxy`, замена кода на лету, `!!js` в конфигурации, npm как канал установки.
3. **Гарантии теории условны и рантаймом не проверяются.** Честность обратных действий и коммутативность ключей лежат на авторе плагина. Если автор task-плагина есть локальная модель, эти обязательства нельзя оставлять автору: сгенерированный плагин должен состоять из регистраций, обратные действия которых написаны нами (декларативный bundle), а не из произвольного кода.
4. **Внешние эффекты вне теории.** Отправленное письмо, запись в чужую систему, запущенный чужой harness не откатываются выгрузкой плагина. Для них нужен отдельный слой: журнал действий, подтверждения, компенсации. В dsh этот слой состоит из журнала сессии и разовых подтверждений; для нашего продукта этого мало.
5. **Один жизненный цикл для всех плагинов.** dsh пробовал отдельный цикл для сгенерированного кода и отказался. Task-плагин должен устанавливаться, выключаться и удаляться тем же путём, что и поставляемые.
6. **Изоляции в образце нет.** Плагин dsh исполняется с правами хоста. Если task-плагины содержат код, граница нужна с первого дня: встроенный интерпретатор с узким набором возможностей либо WASM, либо отдельный процесс.
7. **Журнал сессии как единственный источник истины.** Правило «видимое модели записано в журнал» даёт воспроизведение, восстановление после сбоя, тесты без ключа и аудит. Его стоит принять как закон ядра раньше, чем писать цикл.
8. **Цикл агента обязан оставаться плагином с пятью точками перехвата.** Разрешения, sandbox, compaction, retry, цели не должны попадать внутрь цикла. Тогда чужой harness можно подключить как альтернативного исполнителя за тем же интерфейсом `Agent`.
9. **Проверка `inject` на этапе компиляции есть реальное преимущество Rust**, а не косметика: главный опубликованный инцидент dsh был именно потерей объявления зависимости.
10. **Интерфейс писать свой**, внешний контракт держать на уровне протокола SDK (JSON-RPC по stdio или сокету). Это заодно даёт способ проверять соответствие снаружи процесса.

## 9. Неожиданности

- Теории категорий в статье по существу нет: аппарат это моноиды и операционное исчисление. Категорная рамка принадлежит пересказам.
- Статья сама называет применение к агентным harness будущей проверкой. Единственный кейс это чат-бот-фреймворк Koishi на предыдущей версии ядра.
- В статье есть раздел о других языках, и для Rust там названы трейты, процедурные макросы вместо `Proxy`, `dlopen` и освобождение экземпляра Wasmtime.
- Сопровождающие dsh предлагают убрать замену кода на лету и оставить перекомпоновку конфигурации.
- dsh уже запускает Claude Code и Codex как субагентов, а любого ACP-агента через общий провайдер; но только одноразово, без возобновления и без потока прогресса.
- От исполнения сгенерированного моделью кода в `node:vm` проект отказался (заметка от 2026-09-16) и заменил его записью обычных пакетов плагинов.
- Встроенной памяти агента нет; есть три выключенных примера сторонних MCP-серверов.
- Тестов больше, чем кода (908 тыс. строк против 663 тыс.), покрытие 100 % обязательно, и при этом опубликованы инциденты, которые оно пропустило.
- Rust-аналог «всё есть плагин» уже существует (rig-harness на Bevy), и он требует toolchain и пересборки хоста при любом изменении состава.
- Ядро вендорено с 28 локальными правками, включая исправления реентерабельного освобождения: upstream на стадии release candidate и сам предупреждает о нестабильном API.

## 10. Открытые вопросы

Вопросы 3–5 пересекаются с соседними отчётами в том же каталоге (`dgx-spark-qwen.md`, `harness-adapters.md`); с ними этот отчёт не сверялся.

1. **Степень совместимости.** «По образу» означает общую архитектуру или совместимость форматов: журнал сессий V4, протокол SDK, YAML-строки конфигурации? От ответа зависит, применим ли корпус снимков dsh как проверка соответствия.
2. **Чужие плагины dsh.** Нужно ли исполнять существующие JavaScript-плагины экосистемы dsh? Если да, потребуется Node рядом с ядром, и преимущество единого бинарника теряется.
3. **Сервер модели у заказчика.** Какой сервер отдаёт Qwen (vLLM, llama.cpp, Ollama, NIM), поддерживает ли он вызовы инструментов в стриминге, какой формат рассуждений (`qwen` или `qwen-chat-template`), какое окно контекста. От этого зависит таблица `compat` и настройки compaction. Не проверено.
4. **Платформа первой поставки.** DGX Spark по общедоступным сведениям это aarch64 Linux; в исследовании не проверялось. Нужно подтвердить доступность Landlock и bubblewrap в его системе и принять, что Wasmtime на aarch64 Linux относится к Tier 2.
5. **Управление чужими harness.** Какой протокол у каждого из четырёх (Claude Code, Codex, OpenCode, Qwen Code) даёт возобновление сессии, поток событий и ответы на запросы разрешений? Поддержка ACP каждым из них известна только по вторичным источникам.
6. **Форма task-плагина.** Что входит в манифест задачи, как устроены память и источники, кто и когда решает, что задача стала «видом задачи». Прецедента в dsh нет.
7. **Граница отката.** Какие действия агента считаются внутренними (откатываются выгрузкой), а какие внешними (только компенсация и подтверждение). Статья оставляет это проектировщику.
8. **Версия образца.** На что ссылаться как на эталон семантики ядра: вендоренная копия dsh (rc.12 с правками V1–V28) или upstream rc.13. Они расходятся, и обе не стабильны.
9. **Windows.** Требование кроссплатформенности включает sandbox на Windows уже в первой версии или позже? В dsh это отдельный пакет с частичной гарантией.
10. **Лицензии.** dsh и Cordis под MIT; у репозитория статьи лицензия не указана (по GitHub API). Если переносятся тексты промптов, схемы инструментов или тесты, нужно сохранить уведомление MIT.

## Приложение A. Полная опись пакетов

Отметки в колонках слоёв: «да» означает безусловную строку, «усл.» строку с условием `!!js`, «выкл.» выключенную строку. Пустая ячейка: пакет строкой не монтируется (может использоваться как библиотека). Назначение взято из поля `description` в `package.json`.

#### `apps` — пакетов: 12, исходники ≈ 30190 строк, тесты ≈ 128029 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `apps/cli` | 1081 | 16483 |  |  |  |  | dsh CLI: profile launch, plugin management, and configuration inspection |
| `apps/desktop` | 23756 | 27805 |  |  |  |  | Electron desktop shell for a bundled dsh runtime and external plugins |
| `apps/desktop-host` | 627 | 612 |  |  |  |  | Private Node-mode host process for the Electron desktop application |
| `apps/desktop-publisher` | 1997 | 1641 |  |  |  |  | Private Desktop release metadata, rollout and website publisher |
| `apps/plugin-market-generator` | 1453 | 1085 |  |  |  |  | Reference generator of plugin market index files from a list of npm packages and Git repositories |
| `apps/web` | 893 | 80403 |  |  |  |  | Web application entry: vite build over the @deepseek-ai/dsh-client-web shell library; dist/ served by apps/cli… |
| `apps/web/tests/fixtures/plugins/fixture-bundle` | 9 | 0 |  |  |  |  | Registry description for the fixture bundle. |
| `apps/web/tests/fixtures/plugins/fixture-forge-extension` | 144 | 0 |  |  |  |  |  |
| `apps/web/tests/fixtures/plugins/fixture-icon-skin` | 23 | 0 |  |  |  |  |  |
| `apps/web/tests/fixtures/plugins/fixture-input-extension` | 48 | 0 |  |  |  |  |  |
| `apps/web/tests/fixtures/plugins/fixture-layout-bottom` | 32 | 0 |  |  |  |  |  |
| `apps/web/tests/fixtures/plugins/fixture-live-client` | 127 | 0 |  |  |  |  |  |

#### `native` — пакетов: 6, исходники ≈ 1772 строк, тесты ≈ 987 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `native/system` | 1126 | 987 |  |  |  |  |  |
| `native/system/packages/darwin-arm64` | 0 | 0 |  |  |  |  | Prebuilt POSIX flock Node-API binding for macOS arm64 |
| `native/system/packages/darwin-x64` | 0 | 0 |  |  |  |  | Prebuilt POSIX flock Node-API binding for macOS x64 |
| `native/system/packages/entry` | 646 | 0 |  |  |  |  | Prebuilt system primitives: a Linux Landlock launcher and asynchronous POSIX flock through stable Node-API |
| `native/system/packages/linux-arm64` | 0 | 0 |  |  |  |  | Linux arm64 system binaries: static Landlock launcher and glibc/musl Node-API flock addons |
| `native/system/packages/linux-x64` | 0 | 0 |  |  |  |  | Linux x64 system binaries: static Landlock launcher and glibc/musl Node-API flock addons |

#### `packages/acp` — пакетов: 1, исходники ≈ 1782 строк, тесты ≈ 3583 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `acp` | 1782 | 3583 |  |  |  |  | Automation-only Agent Client Protocol server for driving DeepSeek Harness agents over JSON-RPC stdio |

#### `packages/api` — пакетов: 9, исходники ≈ 24583 строк, тесты ≈ 47090 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `gateway` | 5674 | 12104 | да |  |  |  | Typert Remote Host dispatcher and Client API endpoint |
| `job-controller` | 1001 | 1362 |  | да |  |  | Job Remote observation stream and the reference-counted client job-output service |
| `remote-access` | 1080 | 1135 | да |  |  |  | Method-level Remote access control for non-owner Peers and the per-Peer access snapshot |
| `remotes` | 479 | 601 |  | да |  |  | Remote BFF assembly for application-selected Host capabilities |
| `session-controller` | 9423 | 22129 |  | да |  |  | Session Remote commands, cold reads, and live control transport |
| `settings-controller` | 461 | 245 |  | да |  |  | Remote owner for the configuration surfaces over the settings-domain seams |
| `terminal-controller` | 2864 | 3805 |  | да |  |  | Session-owned interactive terminals with shell discovery, screen recovery and typed Remote control |
| `workspace-controller` | 1970 | 1973 |  | да |  |  | Workspace Remote commands and reconnect-safe state transport |
| `workspace-files` | 1631 | 3736 |  | да |  |  | Workspace file service and Client resource provider: bounded reads, directory listing, and live metadata over … |

#### `packages/attachment` — пакетов: 2, исходники ≈ 2269 строк, тесты ≈ 2286 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `attachment` | 710 | 476 |  |  |  |  | Durable immutable attachment storage seam for the DeepSeek Harness |
| `attachment-local` | 1559 | 1810 | да |  |  | да | Private content-addressed DSH_HOME attachment storage |

#### `packages/boot` — пакетов: 6, исходники ≈ 13497 строк, тесты ≈ 22436 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `app-boot` | 5459 | 9882 |  |  |  |  | Shared boot glue for the app bins: .env loading, fail-loud Loader guards, snapshot-aware config resolution, an… |
| `cmdline` | 239 | 371 |  |  |  |  | Immutable command-line handoff from a dsh launcher to any app plugin that injects cmdlineArgs |
| `config-editor` | 155 | 0 | усл. |  |  |  | Persist plugin configuration through profile patches and Loader reconciliation |
| `hmr` | 1740 | 4002 | усл. |  |  |  | Coordinated module and profile configuration hot reload |
| `plugin-manager` | 3934 | 6197 | выкл. |  | усл. |  | Current-profile plugin and bundle management shared by dsh CLI, Web and agent tools |
| `plugin-market` | 1970 | 1984 |  |  | выкл. |  | Configured plugin market indexes for the Web Discover page and the plugin_market agent tool |

#### `packages/browser-use` — пакетов: 1, исходники ≈ 65 строк, тесты ≈ 51 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `browser-use` | 65 | 51 |  |  |  |  | Exclusive named browser-use provider registration |

#### `packages/bundle` — пакетов: 6, исходники ≈ 1548 строк, тесты ≈ 4073 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `acp-app` | 48 | 97 |  |  |  |  | The dsh ACP profile bundle: automation-only JSON-RPC stdio and process lifecycle over dsh-base |
| `base` | 9 | 84 |  |  |  |  | The shared dsh core as a profile bundle: the first patch layer of base-backed profiles, inserting core rows ov… |
| `headless` | 992 | 2218 |  |  |  |  | The dsh one-shot bundle: a direct core Agent/Session runner over dsh-base with no Host, HTTP, or browser layer |
| `sdk-app` | 62 | 100 |  |  |  | да | The dsh SDK profile bundle: stdio JSON-RPC serving and process lifecycle over dsh-base |
| `sdk-minimal` | 9 | 92 |  |  |  |  | The standalone minimal SDK profile bundle: JSON-RPC, one DeepSeek adapter, persistent shell, and JSONL session… |
| `web-app` | 428 | 1482 |  | да |  |  | The dsh browser-surface bundle: the web patch layer over dsh-base plus the runtime glue plugin (frontend dist … |

#### `packages/client` — пакетов: 74, исходники ≈ 217763 строк, тесты ≈ 218092 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `connection` | 2307 | 3958 |  | да |  |  | Authenticated RPC transport and generation lifecycle |
| `file-upload` | 978 | 693 |  | да |  |  | Agent-scoped browser file upload, streaming intake, and staged receipt service |
| `hmr` | 349 | 489 |  | да |  |  | Web client graph synchronization and rebuilt-bundle reload transport |
| `locale` | 906 | 1112 |  | да |  |  | Locale plugin: Host-backed preference, extensible language catalog, browser fallback, and typed built-in dicti… |
| `modules` | 2911 | 4053 |  | да |  |  | Client module system, dual-face: node half composes the __DSH_BOOT__ entry graph (incremental dsh.client scan,… |
| `product-analytics` | 498 | 670 |  | усл. |  |  | Desktop product event collection and authenticated Host reporting |
| `resources` | 374 | 610 |  | да |  |  | Unified client resource model: protocol-registered providers turn URL addresses into live values, consumed thr… |
| `script-realm` | 133 | 80 |  |  |  |  | Direct-reference script environments and Cordis-owned provider registration |
| `script-realm-iframe` | 192 | 237 |  |  |  |  | Optional iframe ScriptRealm provider |
| `shortcuts` | 1252 | 1468 |  | да |  |  | Application keyboard command registry and physical-key routing |
| `store` | 399 | 295 |  |  |  |  | React-free observable and snapshot-store contracts with the shared Zustand/Immer engine |
| `ui-account-notifications` | 586 | 1574 |  | да |  |  | Display account notifications and refresh localized content |
| `ui-agent-preset` | 3263 | 3285 |  | да |  |  | Agent-preset surfaces: the default for later sessions, this session's seat, and the composition editor |
| `ui-agent-team` | 1084 | 1054 |  | да |  |  | Web shared task board for a subagent tree |
| `ui-approval` | 848 | 725 |  | да |  |  | Approval composer takeover over the scoped Remote Event waterfall |
| `ui-attachment` | 1536 | 1177 |  | да |  |  | Dynamic attachment presentation plugin for conversation input, message-image, and trajectory image slots |
| `ui-brand-official` | 473 | 534 |  | да |  |  | Official DeepSeek Harness brand artwork and click-triggered home background |
| `ui-chat` | 19115 | 21118 |  | да |  |  | Chat Conversation target, node definitions, renderers, and details surface |
| `ui-commands` | 1995 | 2484 |  | да |  |  | Client command surface: global directory cache, '/' source, three command UI kinds, popupSelect registry |
| `ui-context-menu` | 752 | 953 |  | да |  |  | Typed, plugin-extensible context menus with captured targets and shared Web presentation |
| `ui-conversation` | 19896 | 18624 |  | да |  |  | Target-neutral Conversation assembly, shell, composer, queue, and view navigation |
| `ui-deliverables` | 2789 | 2686 |  | да |  |  | Changed-files card with per-file comparison tabs, delivery cards, and clickable final-response file references… |
| `ui-directory-picker-browse` | 1752 | 2012 |  |  |  |  | In-app directory browsing surface: the workspace directory-flow owner rendering the host's listing and creatio… |
| `ui-directory-picker-native` | 124 | 364 |  |  |  |  | Native directory-picker surface: the renderless workspace directory-flow occupant driving the local Desktop or… |
| `ui-dockkit` | 5499 | 4870 |  |  |  |  | Docking layout kit: split-tree engine with invertible operations, and the React components that render and dri… |
| `ui-entry-customization` | 261 | 291 |  | да |  |  | Customize plugin entries in the left sidebar, conversation views, and right sidebar, and quickly create new pl… |
| `ui-file-actions` | 427 | 318 |  | да |  |  | Original-file saving and image clipboard actions for Web and Desktop |
| `ui-file-drag` | 277 | 304 |  | да |  |  | Application-owned file drag transfers and localized failure feedback |
| `ui-goal` | 1036 | 1189 |  | да |  |  | Session goal surface: GoalBar docked above the composer, read from the goal session projection |
| `ui-input-trigger` | 2120 | 2204 |  | да |  |  | Input trigger pipeline: '/' and '@' detection, candidate menu, pick routing to registered sources |
| `ui-jobs` | 1547 | 1085 |  | да |  |  | Session-header background-job list with on-demand streaming record panels |
| `ui-layout` | 1466 | 1805 |  | да |  |  | Shell plugin: three-column AppFrame with drag handles, ctx.layout viewing-state service (navigation + panels) |
| `ui-message-feedback` | 1336 | 1640 |  | да |  |  | The Web feedback surface: per-message Like/Dislike in the assistant-message action strip and the feedback dial… |
| `ui-model-selection` | 2065 | 1920 |  | да |  |  | Model selection over the shared model catalog, Session projection, and session.selectModel |
| `ui-open-in-app` | 1665 | 2098 |  | да |  |  | Session application submenus and native opening controls for directories and previewed files |
| `ui-permission-presets` | 1444 | 1318 |  | да |  |  | Permission surfaces: a new-session default in General settings and a current-session /permission popup over th… |
| `ui-plan` | 907 | 850 |  | да |  |  | Plan mode controls, persistent transcript plan cards, and sidebar Markdown previews |
| `ui-plugin-manager` | 9888 | 6812 |  | да |  |  | Plugin management for the dsh web client: the sidebar Plugins panel installs, enables, disables, retries, and … |
| `ui-plugin-market` | 2485 | 1196 |  |  |  |  | Discover tab of the dsh web Plugins page: browse configured plugin market indexes and install their bundles |
| `ui-primitives` | 18441 | 15477 |  |  |  |  | Pure React atoms for the dsh web UI: controls, icons, markdown, and JSON inspectors (zero cordis) |
| `ui-reference` | 604 | 872 |  | да |  |  | Unified Web @file and @session reference source |
| `ui-renderer` | 2982 | 5148 |  | да |  |  | Browser UI renderer: React slot bindings, ctx.uiRenderer, and the assembled application root |
| `ui-schedule` | 10279 | 10755 |  | да |  |  | Host task management page and Session reminder catalog |
| `ui-secret-mask` | 975 | 575 |  | да |  |  | Web display masking for recognized credentials in sent user text |
| `ui-session` | 677 | 700 |  | да |  |  | Session Controller adapter for React and session-scoped slots |
| `ui-session-status` | 742 | 1492 |  | да |  |  | Client Session status table over Host reading records: running state, the effective pending interaction, and r… |
| `ui-settings` | 1074 | 1141 |  | да |  |  | Settings domain base plugin: shared configuration forms and the canonical settings slot-type contract |
| `ui-settings-account` | 3462 | 4548 |  | да |  |  | Manage DeepSeek login and open Platform billing pages |
| `ui-settings-agent-loop` | 313 | 230 |  | да |  |  | Settings page of the agent loop on the dsh web client's Plugins page: the parallel tool-call cap of the agent-… |
| `ui-settings-general` | 2598 | 2204 |  | да |  |  | Settings ownerless-copy and product onboarding plugin: the General section, shell trigger/header chrome conten… |
| `ui-settings-models` | 7522 | 7616 |  | да |  |  | Models settings and shared product-onboarding dialogs over existing settings and credential joins |
| `ui-settings-plugin-inventory` | 1501 | 875 |  | да |  |  | Read-only Cordis Loader inventory tab in Web Plugins settings |
| `ui-settings-plugins` | 414 | 191 |  | да |  |  | Built-in plugins settings section for the dsh web client: the Settings navigation entry and the tab chrome fea… |
| `ui-settings-session-log` | 231 | 138 |  | да |  |  | General settings control for Session-log upload with DeepSeek API requests |
| `ui-settings-shell` | 351 | 354 |  | да |  |  | Settings page of the shell executor on the dsh web client's Plugins page: the command timeout and the per-stre… |
| `ui-settings-subagent` | 1379 | 1035 |  | да |  |  | Settings page of Subagent delegation on the dsh web client's Plugins page: recursion depth, parallel capacity,… |
| `ui-settings-web-search` | 1605 | 1697 |  | да |  |  | Settings page for web search and fetch on the dsh web client's Plugins page: the Search and Fetch provider men… |
| `ui-shortcuts` | 1117 | 1311 |  | да |  |  | Keyboard shortcut reference, recording, and local preference editing |
| `ui-sidebar` | 2064 | 2225 |  | да |  |  | Sidebar plugin: session multi-level tree, search, grouping, state dots |
| `ui-sidebar-browser` | 2062 | 1314 |  | усл. |  |  | Sandboxed Web browser tabs for the right Sidebar |
| `ui-sidebar-documentpreview` | 7382 | 9238 |  | да |  |  | Extensible Sidebar previews for Office documents, spreadsheets, Markdown, code, images, PDF, HTML, and plain t… |
| `ui-sidebar-files` | 4694 | 5879 |  | да |  |  | The workspace files page and one workspace directory tree per pane beside docked and floating file previews, l… |
| `ui-sidebar-right` | 5130 | 5710 |  | да |  |  | Right Sidebar: the docking surface's session-bound state, its panel and header expand control, and the navigat… |
| `ui-sidebar-terminal` | 1064 | 1236 |  | да |  |  | Interactive shell tabs for the right Sidebar |
| `ui-skill` | 830 | 888 |  | да |  |  | Web skill references and the dedicated skill tool row |
| `ui-slots` | 2057 | 914 |  |  |  |  | Slot registry pure core: typed ordinary Slots and reusable Component Factories, derived props, Store seats, an… |
| `ui-subagent` | 2269 | 2453 |  | да |  |  | Subagent conversation catalog, continuation routing UI, and '@' reference source |
| `ui-theme` | 2968 | 3741 |  | да |  |  | Theme plugin: Host bootstrap for the pre-plugin palette; ThemeRuntime for light/dark/system state and icon rep… |
| `ui-tool` | 6277 | 6963 |  | да |  |  | Client Tool call-tree renderer and keyed per-tool presentation slot |
| `ui-trajectory` | 13864 | 6821 |  | да |  |  | Trajectory event ledger and timing overview with live tool-result inspection |
| `ui-user-questions` | 3344 | 3284 |  | да |  |  | Web ask_user_question composer takeover and plan-review presentation UI |
| `ui-workflow-run` | 1198 | 1022 |  | да |  |  | Durable workflow-run Conversation Node and nested member disclosure for dsh web |
| `ui-workspace` | 8206 | 10541 |  | да |  |  | Workspace picker plugin: one WorkspacePicker registered into the sidebar and empty-state workspace slots |
| `web` | 1152 | 1349 |  |  |  |  | Web boot kernel: static module table, Cordis loader, framework-free boot page, and UI-renderer handoff |

#### `packages/compaction` — пакетов: 5, исходники ≈ 2911 строк, тесты ≈ 6478 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `command-compact` | 109 | 469 | да |  | да |  | Human-facing slash command for explicit session compaction |
| `compaction` | 513 | 605 |  |  |  |  | Abstract compaction service seam (ctx.compaction) for the DeepSeek Harness |
| `compaction-basic` | 1789 | 4434 | да |  | да | да | Token-meter-driven compaction policy and LLM summarization backend for the DeepSeek Harness |
| `compaction-image-offload` | 197 | 614 | да |  |  | да | Durable image offload for image-capable routes: replace over-budget request images with placeholders and retry |
| `compaction-tool-result-pruner` | 303 | 356 | да |  | да |  | Replay-safe model-free head/middle/tail pruning for tool-result surface nodes |

#### `packages/computer-use` — пакетов: 1, исходники ≈ 65 строк, тесты ≈ 51 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `computer-use` | 65 | 51 |  |  |  |  | Exclusive named computer-use provider registration |

#### `packages/context` — пакетов: 7, исходники ≈ 4582 строк, тесты ≈ 10373 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `agent-instructions` | 2114 | 6711 | да |  | да |  | Workspace context loader for AGENTS.md/CLAUDE.md instruction files |
| `file-reference` | 112 | 21 |  |  |  |  | File-reference discovery contract and shared @file grammar |
| `file-reference-local` | 523 | 541 |  | да |  |  | Local-filesystem ctx.fileReferences provider with bounded fuzzy indexes |
| `harness-dev-context` | 158 | 231 |  | да |  |  | Model context naming the DeepSeek Harness checkout and Web GUI URL without letting either change the system pr… |
| `session-reference` | 1011 | 1484 |  | да |  |  | Cross-session snapshot references and durable untrusted model context (ctx.sessionReferenceResolver) |
| `time-context` | 392 | 901 |  |  | да |  | Durable per-step context with the current time and elapsed time |
| `tmux-context` | 272 | 484 |  |  |  |  | Opt-in durable per-step context with this agent's tmux pane and window location |

#### `packages/core` — пакетов: 10, исходники ≈ 14253 строк, тесты ≈ 35117 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `agent` | 1806 | 1677 | да |  |  | да | Agent interface, registry, initiator scope, and event vocabulary for the DeepSeek Harness |
| `agent-default-model` | 113 | 70 | да |  |  |  | Default model selection shared by Agent entry points |
| `agent-loop` | 2164 | 15448 | да |  |  | да | The concrete agent loop plugin for the DeepSeek Harness |
| `agent-tool-presentation` | 54 | 139 |  |  |  |  | Deprecated tool presentation selector; use dsh-tool-executor-native or dsh-tool-executor-ptc directly |
| `scope` | 486 | 511 |  |  |  |  | Scoped-context registration primitive (scope tags, scope-filtered event dispatch) for the DeepSeek Harness |
| `session` | 3500 | 6855 | да |  |  | да | Event-sourced session store for the DeepSeek Harness |
| `system-prompt` | 699 | 1167 | да |  |  | да | System prompt assembly registry for the DeepSeek Harness |
| `tool-executor-native` | 35 | 0 | да |  |  | да | Native tool execution plugin |
| `tool-executor-ptc` | 1586 | 3851 | выкл. |  |  |  | PTC program execution, tool bindings, and generated SDK prompts |
| `tools` | 3810 | 5399 | да |  |  | да | Tool registry and execution pipeline for the DeepSeek Harness |

#### `packages/credentials` — пакетов: 4, исходники ≈ 3410 строк, тесты ≈ 5387 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `authorization` | 541 | 603 | да |  |  |  | Authorization seam (ctx.authorization): plugin-owned flows that obtain a credential through a conversation wit… |
| `credentials` | 409 | 176 |  |  |  |  | Abstract credential seam (ctx.credentials): settings carry references to secrets, providers own the values |
| `credentials-local` | 930 | 1577 | да |  |  |  | File-backed credentials provider ($DSH_HOME/.env under the live process environment) for the DeepSeek Harness |
| `deepseek-account` | 1530 | 3031 | да |  |  |  | Sign in to DeepSeek and manage account credentials and notifications |

#### `packages/deliverables` — пакетов: 3, исходники ≈ 1451 строк, тесты ≈ 1801 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `deliverable-file-references` | 30 | 42 |  | да |  |  | Shared model guidance for file links and deliverables across DeepSeek Harness hosts |
| `tool-present` | 127 | 282 |  |  | да |  | Explicit workspace file delivery declarations for the DeepSeek Harness |
| `workspace-changes` | 1294 | 1477 |  | да |  |  | Per-turn workspace file changes recorded from git working-tree snapshots and whole-file captures, with per-fil… |

#### `packages/document` — пакетов: 2, исходники ≈ 2160 строк, тесты ≈ 3129 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `office-kit` | 1399 | 2071 |  | да |  |  | Shared Office rendering, conversion, and recalculation with bounded queues and caching |
| `tool-office` | 761 | 1058 |  |  | да |  | Model-facing Office tools for rendering, converting, and recalculating documents |

#### `packages/experimental` — пакетов: 65, исходники ≈ 142460 строк, тесты ≈ 154968 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `action-cards` | 674 | 645 |  |  |  |  | Optional action cards below answers: one shared proposal tool, card state, user actions and status notices |
| `api-speech-to-text` | 139 | 104 |  |  |  |  | Authenticated experimental speech transcription for browser clients |
| `auto-review` | 872 | 2499 |  |  |  |  | Per-tool LLM authorization review for the DeepSeek Harness Auto permission preset |
| `badge-skill-bundle` | 2 | 0 |  |  |  |  | Let agents add a “powered by dsh” badge to documents, pull requests, and other outputs. |
| `browser-use-chrome-devtools-mcp` | 43 | 45 |  |  |  |  | Experimental per-Session Chromium browser tools through chrome-devtools-mcp |
| `browser-use-playwright-mcp` | 51 | 47 |  |  |  |  | Experimental per-Session Chromium browser tools through @playwright/mcp |
| `browser-use-runtime` | 471 | 1418 |  |  |  |  | Session-owned browser resource lifecycles and MCP integration for experimental providers |
| `browser-use-stagehand-native` | 747 | 1470 |  |  |  |  | Experimental Stagehand browser tools with separately configured native models |
| `claude-code-mods` | 4206 | 3222 |  |  |  |  | Experimental bridge: load Claude Code mods (hooks modules) and run their hook chains on DeepSeek Harness exten… |
| `client-ui-action-cards` | 479 | 272 |  |  |  |  | Action cards below completed answers: the shared card frame, kind bodies and the compact tool row |
| `client-ui-claude-code-mods` | 468 | 296 |  |  |  |  | Web band above the prompt for Claude Code mods: draws each session's mod tree and sends button presses back to… |
| `client-ui-cot-translation` | 783 | 1180 |  |  |  |  | Optional machine translation of expanded reasoning, preserving original Session text |
| `client-ui-creation-recommendations` | 634 | 316 |  |  |  |  | Body of the personalized page card: location choice, creation progress, Undo and Restore |
| `client-ui-forge` | 22531 | 17704 |  |  |  |  | Sidebar panel for a Session's change requests, with a worktree option for new Sessions and its Host Remote |
| `client-ui-plugin-recommendations` | 475 | 310 |  |  |  |  | Official plugin card body with catalog text and enabling |
| `client-ui-stats` | 7242 | 5576 |  |  |  |  | Per-turn and per-step context, timing, cache, and tool-cost table contributed as an experimental conversation … |
| `client-ui-voice-input` | 1829 | 1666 |  |  |  |  | Record speech and insert editable text into the conversation draft |
| `computer-use-cua-driver-mcp` | 72 | 401 |  |  |  |  | Experimental computer use through an installed Cua Driver MCP executable |
| `computer-use-cua-driver-native` | 182 | 536 |  |  |  |  | Experimental computer-use provider embedding the Cua Driver native npm SDK |
| `cot-translation-bundle` | 5 | 0 |  |  |  |  | Experimental machine translation of expanded reasoning |
| `creation-recommendations` | 567 | 1270 |  |  |  |  | Create persistent removable interface plugins from explicit conversation needs |
| `cua` | 4124 | 5883 |  |  |  |  | Shared Computer Use application objects, authorization, and logged tool execution |
| `cua-bundle` | 2 | 36 |  |  |  |  | Optional Computer Use for macOS and Windows |
| `cua-macos` | 6635 | 11313 |  |  |  |  | macOS native provider for shared Computer Use application and window objects |
| `cua-mcp` | 557 | 1268 |  |  |  |  | Computer Use as an MCP stdio server: a dsh profile bundle that exposes cua_run_code to external agent harnesse… |
| `cua-windows` | 716 | 1308 |  |  |  |  | Windows C# computer use through shared application and window objects |
| `deepseek-chat-import` | 3091 | 3050 |  |  |  |  | Import DeepSeek chat exports as stored Sessions in a chosen Workspace |
| `forge` | 1394 | 1367 |  |  |  |  | Provider-neutral change request registry with shared status polling |
| `forge-bundle` | 5 | 0 |  |  |  |  | Experimental change request panel and worktree option for Sessions in Git repositories |
| `forge-github` | 2573 | 2854 |  |  |  |  | GitHub provider for the experimental forge registry, read through the GitHub CLI on the host |
| `forge-session` | 944 | 2013 |  |  |  |  | Binds change requests to the Session that created, pushed, or tracked them |
| `hook-protocol` | 759 | 754 |  |  |  |  | Shared Claude Code / Codex hook wire protocol: matcher engine, stdin/exit-code/stdout codec, multi-hook merge,… |
| `hooks-claude-code` | 496 | 1390 |  |  |  |  | Bridge plugin: run a Claude Code hooks.json / settings hook config on the DeepSeek Harness interception seams |
| `hooks-codex` | 422 | 1054 |  |  |  |  | Bridge plugin: run a Codex hooks.json hook config on the DeepSeek Harness interception seams |
| `inspector` | 16730 | 7889 |  |  |  |  | Experimental cross-realm CDP hub for Host debugging and Client Runtime inspection |
| `inspector-profile` | 3 | 30 |  |  |  |  | Optional Web bundle for raw Session logs and Chat node inspection |
| `liveshare` | 21238 | 36991 |  |  |  |  | Opt-in LiveShare host bundle: share a Session through a separate Relay at the view or bounded-write tier, with… |
| `liveshare-protocol` | 1195 | 1447 |  |  |  |  | Liveshare wire frames and the identity contract shared by the owner bundle and the Relay |
| `personalized-recommendations` | 5 | 145 |  |  |  |  | Personalized pages and built-in plugin recommendations offered as cards below answers |
| `plugin-market-bundle` | 5 | 0 |  |  |  |  | Experimental custom plugin indexes: a Discover tab that browses and installs bundles from plugin indexes |
| `plugin-recommendations` | 255 | 524 |  |  |  |  | Official plugin suggestions with explicit user activation and persistent conversation decisions |
| `ralph-bundle` | 2 | 0 |  |  |  |  | Run explicitly requested repeated work with fresh agents. |
| `session-inspector` | 3379 | 3536 |  |  |  |  | Experimental virtualized Session log and live Chat group/node inspectors |
| `session-search` | 2 | 0 |  |  |  |  | Give agents tools to search and read earlier sessions from the current workspace. |
| `session-title-all-prompts-llm` | 177 | 337 |  |  |  |  | All-user-messages LLM provider plugin for DeepSeek Harness session titles |
| `session-titles-bundle` | 2 | 0 |  |  |  |  | Refresh session titles as new human prompts arrive. |
| `skill-badge` | 60 | 53 |  |  |  |  | Bundled dsh badge skill provider for DeepSeek Harness |
| `speech-to-text` | 373 | 271 |  |  |  |  | Experimental speech recognition with independently selectable providers |
| `speech-to-text-sensevoice` | 925 | 1359 |  |  |  |  | Local SenseVoice ONNX transcription with a managed sherpa-onnx process |
| `subagent-oar` | 471 | 985 |  |  |  |  | Experimental one-shot subagents through OAR runtimes |
| `terminal-bundle` | 2 | 0 |  |  |  |  | Let the agent run long-lived commands in terminals you can watch in the sidebar. |
| `terminal-panel` | 755 | 921 |  |  |  |  | Persistent Agent terminals shown in the Web sidebar, plus read-only access to the user's terminals |
| `timeline` | 10849 | 10162 |  |  |  |  | Replace Todo in long-running sessions with task trees, a live progress bar, and timed reports |
| `tool-ralph` | 524 | 729 |  |  |  |  | Model-facing fresh-agent Ralph loop over the workflow and subagent seams |
| `tool-session-query` | 1413 | 2367 |  |  |  |  | Workspace-authorized model-facing session history search, trace, and event read tools |
| `tool-subagent-oar` | 108 | 154 |  |  |  |  | Delegate standalone tasks to configured external subagent providers |
| `tool-terminal` | 622 | 763 |  |  |  |  | Six model-facing persistent PTY tools with owner isolation and generic background-job integration |
| `tool-worktree` | 43 | 116 |  |  |  |  | Model tool that creates and enters a new Git worktree |
| `translator` | 825 | 1955 |  |  |  |  | Machine translation with reusable Session results |
| `voice-input-bundle` | 5 | 0 |  |  |  |  | Experimental voice input with local SenseVoice; downloads its runtime on first use |
| `webhook` | 457 | 658 |  |  |  |  | Fire-and-forget webhook rule runtime that creates Workspace-backed DeepSeek Harness Sessions |
| `webhook-github` | 353 | 458 |  |  |  |  | Signed GitHub HTTP webhook adapter for the DeepSeek Harness webhook runtime |
| `webworker-packer` | 1410 | 910 |  |  |  |  | Build-time packer for the browser runtime's base VFS image and ordered data-overlay archives |
| `webworker-runtime` | 14846 | 9052 |  |  |  |  | Browser-only harness runtime: in-memory VFS, module transform and loader, postMessage tunnel, and the dedicate… |
| `worktree` | 1236 | 1889 |  |  |  |  | Creates a named Git worktree and changes the calling Session working directory |

#### `packages/extensions` — пакетов: 4, исходники ≈ 27507 строк, тесты ≈ 5154 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `cordis-client-runner` | 9610 | 2120 |  | да |  |  | Browser half of dynamic dual-half plugin packages: event subscription, closure evaluation, guard facade, and l… |
| `cordis-host-runner` | 3537 | 2008 |  | да |  |  | Dynamic package definition registry, host-half sandbox lifecycle, and invoke handler table for model-mounted d… |
| `tool-cordis` | 11524 | 488 |  | да |  |  | Read-only runtime API inspection for Harness plugin development |
| `ui-cordis` | 2836 | 538 |  | да |  |  | Cordis dynamic-plugin definition card: the keyed cordis_define tool row with its run/stop switch |

#### `packages/feedback` — пакетов: 2, исходники ≈ 687 строк, тесты ≈ 1449 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `command-feedback` | 197 | 388 | да |  |  |  | Log-only session feedback: the record event, the sessionFeedback Host Remote, and the human-facing slash comma… |
| `message-feedback` | 490 | 1061 |  | да |  |  | Canonical Session-log ratings and notes for finalized assistant messages |

#### `packages/fs` — пакетов: 8, исходники ≈ 7333 строк, тесты ≈ 11068 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `fs` | 640 | 248 |  |  |  |  | Abstract filesystem capability seam (ctx.fs) for the DeepSeek Harness — vocabulary types, the FileSystem servi… |
| `fs-local` | 1401 | 2552 |  |  |  | да | Local-filesystem implementation of the DeepSeek Harness filesystem seam (ctx.fs) |
| `fs-observation-policy` | 170 | 257 | выкл. |  |  |  | File-context policy plugin for the DeepSeek Harness — observed-state, read-before-edit, and version-guarded wr… |
| `fs-patch` | 350 | 334 |  |  |  |  | Pure Codex-style patch parser and LF text updater with structured match failures |
| `fs-sandbox` | 257 | 404 | да |  |  |  | Sandbox-enforcing implementation of the DeepSeek Harness filesystem seam: fences write/edit by the per-call sa… |
| `tool-fs` | 2444 | 4566 | да |  | да | да | Model-facing filesystem tools (read, write, edit) over the DeepSeek Harness filesystem seam (ctx.fs) |
| `tool-fs-search` | 1534 | 2017 | да |  | да |  | Model-facing filesystem discovery tools (glob, grep) backed by the packaged ripgrep binary (@vscode/ripgrep) |
| `tool-str-replace-editor` | 537 | 690 |  |  |  |  | Model-facing view, create, literal replace, and line insert tool over the Harness filesystem service |

#### `packages/goal` — пакетов: 4, исходники ≈ 2531 строк, тесты ≈ 3596 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `command-goal` | 200 | 355 | да |  | да |  | Human-facing slash command for persisted same-session goals |
| `goal` | 1324 | 1190 | да |  |  |  | Event-sourced same-session goal state and lifecycle service for the DeepSeek Harness |
| `goal-round-driver` | 525 | 1300 | да |  |  |  | Race-fenced same-session goal-round driver |
| `tool-goal` | 482 | 751 | да |  | да |  | Model-facing same-session goal tools with execution-time authority checks |

#### `packages/guard` — пакетов: 2, исходники ≈ 315 строк, тесты ≈ 667 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `repeat-tool-reminder` | 234 | 415 | да |  |  |  | Repeat-tool-call guard plugin: advisory reminders when an agent loops on identical tool calls |
| `timeout-policy` | 81 | 252 | да |  |  |  | Tool-call timeout policy: a tools/execute wrapper that arms a per-tool deadline on exec.signal and returns TOO… |

#### `packages/host` — пакетов: 11, исходники ≈ 5277 строк, тесты ≈ 6615 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `authentication-desktop` | 42 | 28 |  |  |  |  | Desktop private-channel authentication provider |
| `authentication-token` | 569 | 762 |  | да |  |  | Default token authentication provider and HTTP cookie exchange |
| `directory-picker` | 154 | 35 |  |  |  |  | Abstract workspace-directory picking seam (ctx.directoryPicker) for the DeepSeek Harness web GUI host |
| `directory-picker-auto` | 217 | 425 |  | да |  |  | Adaptive chooser of the directory-picker seam: resolves the host situation at boot and mounts the native or br… |
| `directory-picker-browse` | 339 | 231 |  |  |  |  | In-app browsing backend of the directory-picker seam (listing/creation primitives over the host filesystem) |
| `directory-picker-native` | 812 | 1075 |  |  |  |  | Native-OS-chooser backend of the directory-picker seam for the DeepSeek Harness web GUI host |
| `frontend-static` | 153 | 421 |  |  |  |  | SPA dist server for the Web shell: owns the webserver fallback seat, serving explicit index entries and static… |
| `open-in-app` | 1681 | 1577 |  | да |  |  | Host half of open-in-app: resolved application catalog, icons, and the launch endpoint as three webServer rout… |
| `plugin-inventory` | 198 | 254 |  | да |  |  | Read-only Remote projection of current Cordis Loader plugin state |
| `product-telemetry-otel` | 121 | 295 |  | усл. |  |  | Explicit product usage events exported through OpenTelemetry HTTP logs |
| `webserver` | 991 | 1512 |  | да |  |  | Web route-registration plugin: HTTP and upgrade routes, per-Peer index transform taps, and static dist fallbac… |

#### `packages/identity` — пакетов: 1, исходники ≈ 100 строк, тесты ≈ 105 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `anonymous-user-id` | 100 | 105 |  |  |  |  | Shared anonymous user identity for DeepSeek Harness telemetry and feedback correlation |

#### `packages/interaction` — пакетов: 5, исходники ≈ 3031 строк, тесты ≈ 4331 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `commands` | 734 | 716 | да |  |  |  | Plugin-owned human command registry for DeepSeek Harness UIs |
| `permission-presets` | 556 | 786 | да |  |  |  | User-facing permission presets (ctx.permissionPresets) for the DeepSeek Harness: one product-level Permissions… |
| `tool-ask-user` | 332 | 698 |  |  | да |  | Model-facing ask_user_question tool over the ctx.userQuestions seam |
| `user-approval` | 445 | 761 | да |  |  |  | User-approval seam (ctx.approval) for the DeepSeek Harness: one-shot permission decisions dispatched to compos… |
| `user-questions` | 964 | 1370 | да |  |  |  | Abstract user-questions seam (ctx.userQuestions) for asking the human during agent runs |

#### `packages/jobs` — пакетов: 3, исходники ≈ 2462 строк, тесты ≈ 4434 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `jobs` | 714 | 312 |  |  |  |  | Background job registry (ctx.jobs) for the DeepSeek Harness — shared ids, owner isolation, polling, cancellati… |
| `jobs-local` | 1050 | 2117 | да |  |  | да | Process-local implementation of the DeepSeek Harness background job registry seam |
| `tool-jobs` | 698 | 2005 | да |  | да |  | Model-facing background job control tools (job_output, job_list, job_kill) over the ctx.jobs registry |

#### `packages/llm` — пакетов: 9, исходники ≈ 15556 строк, тесты ≈ 27190 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `deepseek-llm-api-extensions` | 203 | 182 | да |  |  | да | Additive request-field registry for the official DeepSeek LLM API adapter |
| `llm` | 4245 | 4172 | да |  |  | да | Provider-neutral LLM service interface for the DeepSeek Harness |
| `llm-deepseek` | 3737 | 9025 |  |  |  |  | DeepSeek Messages adapter |
| `llm-deepseek-account` | 196 | 336 | да |  |  |  | DeepSeek account provider authentication and discovery |
| `llm-deepseek-api-key` | 185 | 272 | да |  |  | да | DeepSeek api-key provider authentication and discovery |
| `llm-pi-ai` | 4798 | 8395 | да |  |  |  | pi-ai-backed DeepSeek adapter for the DeepSeek Harness LLM seam (design-verification twin of dsh-llm-deepseek) |
| `llm-retry` | 334 | 1674 | да |  |  | да | Provider-routed LLM request retry policy for the DeepSeek Harness |
| `plugin-package-inventory-deepseek` | 233 | 426 | да |  |  | да | Active Loader-backed plugin package inventory for official DeepSeek LLM API requests |
| `token-meter` | 1625 | 2708 | да |  |  | да | Replay-aware token measurement service (ctx.tokenMeter) for the DeepSeek Harness |

#### `packages/lsp` — пакетов: 3, исходники ≈ 2515 строк, тесты ≈ 3443 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `lsp` | 320 | 188 |  |  |  |  | Abstract LSP capability seam (ctx.lsp) for the DeepSeek Harness — language-server provider registry keyed by b… |
| `lsp-stdio` | 1757 | 2674 |  |  |  |  | Generic stdio language-server provider for the DeepSeek Harness LSP capability seam (ctx.lsp) — spawns configu… |
| `tool-lsp` | 438 | 581 |  |  |  |  | Model-facing lsp tool over the DeepSeek Harness LSP capability seam (ctx.lsp) — one read-only tool with goToDe… |

#### `packages/mcp` — пакетов: 3, исходники ≈ 3050 строк, тесты ≈ 6695 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `mcp-client` | 2175 | 4973 |  |  |  |  | MCP client bridge: connects to MCP servers and registers their tools on ctx.tools |
| `mcp-manager` | 652 | 1349 |  | да |  |  | MCP server manager: a settings-driven fleet of mcp-client instances with runtime status, restart, and probe |
| `mcp-resources` | 223 | 373 | да |  |  | да | Scoped MCP resource discovery and reading through shared model tools |

#### `packages/plan` — пакетов: 1, исходники ≈ 580 строк, тесты ≈ 1737 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `plan-mode` | 580 | 1737 | да |  | да |  | Logged per-agent plan mode with deployment guidance, a direct slash command, and a user-reviewed exit |

#### `packages/preset` — пакетов: 5, исходники ≈ 1211 строк, тесты ≈ 1327 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `agent-preset` | 30 | 271 |  |  | да |  | Declare an Agent capability composition in Cordis YAML |
| `agent-preset-registry` | 1082 | 877 |  | да |  |  | Declarative Agent preset registry and profile-backed editing |
| `agent-preset/skills/dsh-doc/templates/decoration` | 24 | 0 |  |  |  |  |  |
| `agent-preset/skills/dsh-doc/templates/mcp` | 0 | 0 |  |  |  |  |  |
| `persona` | 75 | 179 |  |  | да |  | Composition-authored deployment persona section for the DeepSeek Harness |

#### `packages/ptc-runtime` — пакетов: 2, исходники ≈ 4456 строк, тесты ≈ 7781 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `ptc-runtime` | 364 | 118 |  |  |  |  | Abstract PTC execution seam (ctx.ptcRuntime) for the DeepSeek Harness |
| `ptc-runtime-node` | 4092 | 7663 | да |  |  |  | Node PTC execution with async function, REPL, scoped REPL and VM Script evaluators and optional retained state |

#### `packages/sandbox` — пакетов: 4, исходники ≈ 3998 строк, тесты ≈ 7431 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `sandbox` | 759 | 494 |  |  |  |  | Abstract process-sandbox seam (ctx.sandbox) for the DeepSeek Harness: same-world confinement vocabulary and th… |
| `sandbox-local` | 686 | 1811 | да |  |  | да | Local process-sandbox backends for the DeepSeek Harness sandbox seam: bwrap, the npm-distributed landlock-run … |
| `sandbox-policy` | 343 | 388 | да |  |  | да | Per-call sandbox policy resolver and current model context: deployment fallbacks plus each session's mode and … |
| `sandbox-windows-acl` | 2210 | 4738 |  |  |  |  | Windows ACL write-restriction sandbox backend (restricted-token spawn with capability-SID write allowlist) for… |

#### `packages/schedule` — пакетов: 2, исходники ≈ 4080 строк, тесты ≈ 8010 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `schedule` | 3584 | 7346 |  | да |  |  | Host-wide durable reminders with shared management and delivery to existing or new Sessions |
| `tool-schedule` | 496 | 664 |  |  | да |  | Model-facing scheduled task tools (schedule_create, schedule_list, schedule_view, schedule_edit) over the Host… |

#### `packages/sdk` — пакетов: 3, исходники ≈ 3046 строк, тесты ≈ 7104 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `client` | 1426 | 2134 |  |  |  |  | TypeScript client SDK for driving a DeepSeek Harness runtime subprocess over stdio JSON-RPC: the DeepSeekHarne… |
| `protocol` | 571 | 310 |  |  |  |  | Shared wire protocol for the DeepSeek Harness SDK runtime: the newline-delimited JSON-RPC stdio transport and … |
| `server` | 1049 | 4660 |  |  |  | да | Stdio JSON-RPC server plugin for out-of-process DeepSeek Harness SDK clients |

#### `packages/session` — пакетов: 22, исходники ≈ 19057 строк, тесты ≈ 33198 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `message-quotes` | 227 | 325 |  | да |  |  | Selected-text quotation and Markdown blockquote presentation |
| `session-checkpoint-policy` | 83 | 392 | да |  |  | да | Semantic session durability checkpoints before model requests and tool side effects |
| `session-format` | 958 | 758 |  |  |  |  | Streaming adjacent Session format migration machinery |
| `session-format-catalog` | 167 | 563 |  |  |  |  | Build-static first-party Session format codec and migration catalog |
| `session-format-v0-to-v1` | 2892 | 2509 |  |  |  |  | Frozen released-v0 Session codec and identity migration to v1 |
| `session-format-v1-to-v2` | 1269 | 1989 |  |  |  |  | Frozen released-v1 Session codec and assistant-stream migration to v2 |
| `session-format-v2-to-v3` | 828 | 2057 |  |  |  |  | Streaming system-prompt, canonical-envelope and PTC migration into V3 |
| `session-format-v3-to-v4` | 1714 | 3119 |  |  |  |  | Streaming tool-role migration and delivery validation from Session V3 to V4 |
| `session-log-deepseek` | 396 | 1083 | да |  |  | да | Incremental lossless session-log request extension for the official DeepSeek LLM API |
| `session-persistence` | 636 | 1243 |  |  |  |  | Abstract durable session persistence seam (ctx.sessionPersistence) for the DeepSeek Harness |
| `session-persistence-jsonl` | 5173 | 9581 | да |  |  | да | JSONL durable session persistence backend for the DeepSeek Harness |
| `session-projection` | 1031 | 1930 | да |  |  | да | Session-projection seam: the merge-extensible projection type table, the provider contract, and the ctx.sessio… |
| `session-projection-cache` | 579 | 1304 | да |  |  |  | Persisted projection cache (ctx.sessionProjectionCache): durable per-session checkpoint records on the session… |
| `session-stats` | 299 | 450 |  | да |  |  | Whole-log conversation counts and wall times projection (sessionStats) for the DeepSeek Harness |
| `session-telemetry` | 479 | 910 |  |  |  |  | SessionTelemetryBackend seam for the DeepSeek Harness: session-event capture, projection, redaction, and hando… |
| `session-telemetry-otel` | 278 | 1181 | да |  |  |  | Feedback-authorized Session logs over byte-bounded OpenTelemetry HTTP requests |
| `session-title` | 1027 | 1851 | да |  |  | да | Log-backed session title service and provider registry for the DeepSeek Harness |
| `session-title-first-prompt-llm` | 168 | 551 | да |  |  |  | First-message LLM provider plugin for DeepSeek Harness session titles |
| `session-title-llm` | 291 | 569 |  |  |  |  | Shared LLM generation policy for DeepSeek Harness session-title providers |
| `session-turn-outline` | 229 | 377 |  | да |  |  | Whole-log turn outline projection (turnOutline) for the DeepSeek Harness |
| `tool-working-directory` | 43 | 0 | да |  |  | да | Read and change the active Session working directory |
| `working-directory` | 290 | 456 | да |  |  | да | Session working directories with durable changes and model context |

#### `packages/session-query` — пакетов: 3, исходники ≈ 6042 строк, тесты ≈ 8564 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `session-log-export` | 2020 | 3283 |  | да |  |  | Web Session-log ZIP export and Session Markdown copy |
| `session-query` | 2241 | 3066 |  |  |  |  | Combined session query service contract with concrete reads, traces, and filters |
| `session-query-sqlite` | 1781 | 2215 | да |  |  |  | Concrete ctx.sessionQuery backend with SQLite FTS5 search |

#### `packages/settings` — пакетов: 1, исходники ≈ 704 строк, тесты ≈ 800 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `settings` | 704 | 800 | усл. |  |  |  | Abstract user-settings seam (ctx.settings) for the DeepSeek Harness |

#### `packages/shell` — пакетов: 10, исходники ≈ 4894 строк, тесты ≈ 11528 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `bash-local` | 404 | 589 |  |  |  |  | Local-subprocess implementation of the DeepSeek Harness bash executor seam |
| `bash-sandbox` | 206 | 1480 | усл. |  |  |  | Sandbox-consuming implementation of the DeepSeek Harness bash executor seam (confines every command via ctx.sa… |
| `pwsh-local` | 528 | 840 |  |  |  |  | Local PowerShell implementation of the DeepSeek Harness bash executor seam |
| `pwsh-sandbox` | 320 | 717 | усл. |  |  |  | Sandbox-consuming implementation of the DeepSeek Harness PowerShell executor seam (confines every command via … |
| `shell` | 377 | 125 |  |  |  |  | Abstract bash executor seam (ctx.shell) for the DeepSeek Harness |
| `shell-env` | 255 | 249 | да |  |  |  | Tool-independent managed DSH_* shell environment registry |
| `tool-bash` | 871 | 2569 | усл. |  | усл. |  | Model-facing bash tool with optional generic background-job and sandbox-escalation support |
| `tool-bash-persistent` | 488 | 989 |  |  |  | усл. | Model-facing owner-scoped persistent Bash tool backed by the Harness PTY service |
| `tool-pwsh` | 945 | 2346 | усл. |  | усл. |  | Model-facing pwsh tool over the bash executor seam |
| `tool-pwsh-persistent` | 500 | 1624 |  |  |  | усл. | Model-facing owner-scoped persistent PowerShell tool backed by the Harness PTY service |

#### `packages/skill` — пакетов: 5, исходники ≈ 2947 строк, тесты ≈ 5589 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `skill` | 902 | 1275 | да |  |  |  | Agent skill provider registry for the DeepSeek Harness |
| `skill-filesystem` | 1064 | 1530 | да |  | да |  | Local filesystem skill provider for the DeepSeek Harness |
| `skill-office` | 141 | 915 |  | да |  |  | Bundled Word, PowerPoint, and Excel workflows and Office operation guidance |
| `tool-skill` | 452 | 1240 | да |  | да |  | Model-facing skill loading tool for the DeepSeek Harness |
| `tool-workspace-dependencies` | 388 | 629 |  | усл. |  |  | The load_workspace_dependencies tool: absolute paths into a bundled Python, Node.js, and pnpm payload |

#### `packages/spill` — пакетов: 3, исходники ≈ 1211 строк, тесты ≈ 1959 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `spill` | 138 | 61 |  |  |  |  | Abstract spill storage seam (ctx.spillStore) for the DeepSeek Harness — save oversized tool text and return a … |
| `spill-local` | 727 | 663 | да |  |  |  | Local-filesystem implementation of the DeepSeek Harness spill storage seam (private session-scoped files) |
| `spill-policy` | 346 | 1235 | да |  |  |  | Token-budgeted tool-result retention with recoverable text and image paths |

#### `packages/ssh` — пакетов: 5, исходники ≈ 2058 строк, тесты ≈ 4232 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `fs-ssh` | 126 | 242 |  |  |  |  | Filesystem provider over the shared POSIX SSH helper |
| `sandbox-ssh` | 33 | 89 |  |  |  |  | Remote POSIX sandbox argv provider over the shared SSH helper |
| `ssh` | 1515 | 3089 |  |  |  |  | Shared OpenSSH connection and versioned POSIX remote helper |
| `ssh-helper-runtime` | 39 | 0 |  |  |  |  | Private executable carrier for the POSIX SSH helper and managed workers |
| `subprocess-ssh` | 345 | 812 |  |  |  |  | Subprocess and terminal provider over the shared POSIX SSH helper |

#### `packages/storage` — пакетов: 4, исходники ≈ 2634 строк, тесты ≈ 2398 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `storage` | 364 | 346 | да |  |  |  | Storage hub (ctx.storage): named backend registry plus mounted data-form facilities for the DeepSeek Harness |
| `storage-domain` | 910 | 714 | да |  |  |  | Domain data form (ctx.storage.domain): schema-validated, event-emitting KV domains over storage backends for t… |
| `storage-json` | 844 | 839 | да |  |  |  | JSON file KV storage backend for the DeepSeek Harness storage hub |
| `storage-sqlite` | 516 | 499 |  |  |  |  | SQLite storage backend (kv facet) for the DeepSeek Harness storage hub |

#### `packages/subagent` — пакетов: 9, исходники ≈ 10036 строк, тесты ≈ 27096 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `subagent` | 5057 | 11334 | да |  |  |  | Abstract subagent seam (ctx.subagents): named-provider registry for delegating to child agents |
| `subagent-acp` | 747 | 2116 |  |  |  |  | Out-of-process ACP subagent backend: drives a child agent in a spawned subprocess over the Agent Client Protoc… |
| `subagent-claude-code` | 893 | 2822 |  |  |  |  | One-shot Claude Code subagent provider over the official Agent SDK |
| `subagent-codex` | 1280 | 3811 |  |  |  |  | One-shot Codex subagent provider over the official app-server protocol |
| `subagent-dsh-sdk` | 561 | 1293 |  |  |  |  | Out-of-process SDK subagent backend: drives a child DeepSeek Harness runtime subprocess over stdio JSON-RPC th… |
| `subagent-fork-in-process` | 83 | 347 | да |  |  |  | In-process fork subagent backend: runs a child agent seeded with a prefix of the parent's log |
| `subagent-spawn-in-process` | 59 | 525 | да |  |  |  | In-process spawn subagent backend: runs a fresh child agent on ctx.agents |
| `tool-subagent` | 983 | 3380 | да | да | да |  | Model-facing subagent delegation tool over the ctx.subagents seam |
| `tool-subagent-control` | 373 | 1468 | да |  | да |  | Shared messaging, interruption, and child discovery tools for subagents |

#### `packages/subprocess` — пакетов: 3, исходники ≈ 6922 строк, тесты ≈ 10376 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `subprocess` | 591 | 334 |  |  |  |  | Subprocess seam (ctx.subprocess) for the DeepSeek Harness — managed process groups, bounded spill-backed outpu… |
| `subprocess-local` | 5136 | 8797 | да |  |  | да | Local-subprocess implementation of the DeepSeek Harness subprocess seam |
| `win32-process` | 1195 | 1245 |  |  |  |  | Shared low-level Win32 process, stdio, and Job Object primitives |

#### `packages/team` — пакетов: 2, исходники ≈ 1359 строк, тесты ≈ 2926 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `agent-team` | 1124 | 1862 | да |  |  |  | Shared task DAG and change waits for subagent trees |
| `tool-agent-team` | 235 | 1064 | да |  |  |  | Scoped model-facing Agent Teams tools over ctx.agentTeams |

#### `packages/telemetry` — пакетов: 1, исходники ≈ 475 строк, тесты ≈ 466 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `otel` | 475 | 466 | да |  |  |  | Cordis service for independent ordinary-event and byte-bounded Session-log OTLP channels |

#### `packages/terminal` — пакетов: 2, исходники ≈ 2063 строк, тесты ≈ 4025 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `terminal` | 694 | 634 |  |  |  | да | Persistent PTY session seam for the DeepSeek Harness — owner-scoped ids, backend registry, interactive sends, … |
| `terminal-bash` | 1369 | 3391 |  |  |  | усл. | Persistent shell PTY backend over the DeepSeek Harness subprocess terminal primitive |

#### `packages/test-support` — пакетов: 8, исходники ≈ 11824 строк, тесты ≈ 12106 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `agent-loop-testkit` | 211 | 174 |  |  |  |  | Prerequisite mounting, production AgentLoop drivers, and Inbox stubs for tests |
| `client-runtime` | 3153 | 2586 |  |  |  |  | Browser test runtimes: a jsdom slot bench with test-owned Session and Workspace doubles, and a whole-client ti… |
| `llm-mock-server` | 1027 | 489 |  |  |  |  | Scriptable Messages HTTP/SSE fault server for LLM recovery tests |
| `llm-replay` | 1212 | 2528 |  |  |  |  | Replay LLM plugin: short-circuits llm/stream with model chunks reconstructed from a recorded session JSONL (ke… |
| `loader-smoke` | 375 | 466 |  |  |  |  | Shared subprocess and direct-agent harness for keyless real-Loader example smoke tests |
| `remote-mock` | 935 | 910 |  |  |  |  | Endpoint-named mock for Typert Remote traffic: unary answers and stream scripts per &lt;namespace>/&lt;method>, live… |
| `session-snapshot` | 4819 | 4911 |  |  |  |  | Session-log snapshot core with an ACP protocol adapter, expected-output normalization, and fixture invariants |
| `tool-testkit` | 92 | 42 |  |  |  |  | Tool definition and execution fixtures for tests and benchmarks |

#### `packages/todo` — пакетов: 1, исходники ≈ 271 строк, тесты ≈ 683 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `tool-todo` | 271 | 683 | да |  | да |  | Model-facing todo_write tool over the DeepSeek Harness event-sourced session log |

#### `packages/typert` — пакетов: 11, исходники ≈ 11333 строк, тесты ≈ 9794 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `generator` | 7598 | 7060 |  |  |  |  | TypeScript project analyzer and model-driven Typert artifact generator |
| `generator/tests/fixtures/remote-model` | 0 | 0 |  |  |  |  |  |
| `generator/tests/fixtures/remote-model/packages/domain` | 21 | 0 |  |  |  |  |  |
| `generator/tests/fixtures/remote-model/packages/remote` | 59 | 0 |  |  |  |  |  |
| `generator/tests/fixtures/type-model` | 0 | 0 |  |  |  |  |  |
| `generator/tests/fixtures/type-model/packages/client` | 39 | 0 |  |  |  |  |  |
| `generator/tests/fixtures/type-model/packages/host` | 307 | 0 |  |  |  |  |  |
| `generator/tests/fixtures/type-model/packages/write` | 18 | 0 |  |  |  |  |  |
| `loader` | 507 | 849 | да |  |  |  | Loader integration for generated Typert package contributions |
| `protocol` | 1804 | 1105 |  |  |  |  | Compiler-independent Remote metadata and Typert provider protocols |
| `registry` | 980 | 780 | да |  |  |  | Runtime registry for generated package reflection and Zod schemas |

#### `packages/util` — пакетов: 24, исходники ≈ 8376 строк, тесты ≈ 7629 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `atomic-write` | 293 | 513 |  |  |  |  | Atomic file replacement: exclusive-create random-suffix temp + rename carrying the caller-stated permissions (… |
| `brand` | 39 | 13 |  |  |  |  | Stateless branded primitive types for the DeepSeek Harness |
| `chunked-list` | 58 | 59 |  |  |  |  | Persistent append-only chunked lists with bounded copying and JSON checkpoint validation |
| `code-language` | 233 | 174 |  |  |  |  | Single file-extension to syntax-highlighting language table shared by Client code surfaces and the Host read c… |
| `crypto` | 41 | 37 |  |  |  |  | Zero-dependency browser-safe UUID and byte-encoding helpers |
| `deque` | 131 | 94 |  |  |  |  | Zero-dependency circular deque with amortized constant-time end operations and bounded vacant storage |
| `error` | 22 | 19 |  |  |  |  | Dependency-free HarnessError base class with a stable machine-routable code for the DeepSeek Harness |
| `file-drag` | 76 | 31 |  |  |  |  | Shared file drag names and native transfer requests |
| `home-paths` | 238 | 155 |  |  |  |  | Shared filesystem path helpers for the DeepSeek Harness |
| `http-proxy` | 737 | 921 |  |  |  |  | Process-wide outbound HTTP proxy policy for DeepSeek Harness: resolve it from the launch environment and insta… |
| `launch-environment` | 137 | 87 |  |  |  |  | Immutable DeepSeek Harness launch environment that records which layer supplied each value |
| `lazy-require` | 24 | 27 |  |  |  |  | Caller-relative, success-cached lazy loading for CommonJS-compatible Host dependencies |
| `native-command` | 1052 | 978 |  |  |  |  | Host-native command and path-opening utilities with shell-free execution, cancellation, desktop detection, and… |
| `official-bundles` | 213 | 32 |  |  |  |  | Installation-owned lists of official dsh bundles (shipped profile templates, optional, on-demand, and retired … |
| `output-retention` | 460 | 409 |  |  |  |  | Zero-dependency bounded-retention primitive: ItemRetainer/TextRetainer + neutral notice helpers (what did we k… |
| `package-manifest` | 167 | 0 |  |  |  |  | Shared type declarations for package.json.dsh configuration fields |
| `plugin-detail` | 485 | 252 |  |  |  |  | Plugin detail documents: meta.detail or README selection, image paths, and local and npm CDN file sources |
| `plugin-market-index` | 445 | 312 |  |  |  |  | Plugin market index format 1: JSON validation, relative icon resolution, and pinned install arguments |
| `ripgrep` | 109 | 189 |  |  |  |  | Packaged ripgrep resolution and subprocess streaming helpers |
| `text-search` | 301 | 203 |  |  |  |  | Multilingual fuzzy filtering for in-memory search lists |
| `time` | 1708 | 1250 |  |  |  |  | Strict instant and IANA zone parsing, millisecond spans, and bounded Cron/RRULE recurrence arithmetic |
| `timeout` | 212 | 306 |  |  |  |  | Zero-dependency timeout/deadline primitive: clampTimeout, deadline, timeoutOf, TimeoutReason (timing + classif… |
| `values` | 959 | 1317 |  |  |  |  | Duplicate-install-safe value primitives for the DeepSeek Harness |
| `workspace-path` | 236 | 251 |  |  |  |  | Browser-safe Workspace path and display helpers |

#### `packages/web` — пакетов: 5, исходники ≈ 3898 строк, тесты ≈ 6366 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `tool-web` | 1042 | 1692 | да |  | да |  | Model-facing web tools (web_search, web_fetch) over the DeepSeek Harness web capability seam (ctx.web) |
| `web` | 507 | 394 | да |  |  |  | Abstract web access capability seam (ctx.web) for the DeepSeek Harness — search/fetch provider registry, regis… |
| `web-deepseek-api` | 1052 | 2105 | да |  |  |  | DeepSeek Search and Fetch API providers for the DeepSeek Harness web capability seam (ctx.web) |
| `web-fetch-http` | 601 | 853 | да |  |  |  | Anonymous HTTP(S) fetch provider for the DeepSeek Harness web capability seam (ctx.web) |
| `web-search-deepseek` | 696 | 1322 | да |  |  |  | DeepSeek-backed search provider (native web_search via the Anthropic-compatible API) for the DeepSeek Harness … |

#### `packages/workflow` — пакетов: 3, исходники ≈ 2392 строк, тесты ≈ 3294 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `tool-workflow` | 620 | 743 | да |  | да |  | Model-facing workflow tool: run a JavaScript orchestration script over ctx.workflowEngine |
| `workflow` | 397 | 130 |  |  |  |  | Workflow capability seam: ctx.workflowEngine service, run vocabulary, and workflow/* events |
| `workflow-ptc` | 1375 | 2421 | да |  | да |  | Workflow orchestration in the shared sandboxed Node PTC runtime |

#### `packages/workspace` — пакетов: 1, исходники ≈ 1448 строк, тесты ≈ 1587 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `workspace` | 1448 | 1587 |  | да |  |  | Workspace entity registry (ctx.workspaceRegistry): durable workspace records with validated session attachment… |

#### `python` — пакетов: 1, исходники ≈ 558 строк, тесты ≈ 0 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `python/sdk-runtime` | 558 | 0 |  |  |  |  | Dependency-only deploy root defining the dsh executable shipped by the Python runtime wheel. |

#### `vendor` — пакетов: 9, исходники ≈ 7484 строк, тесты ≈ 0 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `vendor/cordis` | 2881 | 0 |  |  |  |  | Meta-Framework for Modern JavaScript Applications |
| `vendor/cosmokit` | 591 | 0 |  |  |  |  | A collection of common utilities |
| `vendor/group` | 3 | 0 |  |  |  |  | Nested plugin group for cordis |
| `vendor/hmr` | 464 | 0 |  |  |  |  | Hot Module Replacement Plugin for Cordis |
| `vendor/include` | 1004 | 0 |  |  |  |  | Include files in cordis configurations |
| `vendor/loader` | 1240 | 0 |  |  |  |  | Plugin loader for cordis |
| `vendor/logger-console` | 169 | 0 |  |  |  |  | Console logger exporter for cordis |
| `vendor/schemastery` | 979 | 0 |  |  |  |  | Type driven schema validator |
| `vendor/timer` | 153 | 0 | да |  |  | да | Timer service for cordis |

#### `website` — пакетов: 1, исходники ≈ 2473 строк, тесты ≈ 1506 строк

| Пакет (путь) | Строк src | Строк тестов | base | web-app | standard | sdk-minimal | Назначение (из package.json) |
|---|---:|---:|---|---|---|---|---|
| `website` | 2473 | 1506 |  |  |  |  |  |

## Приложение B. Каталог инструментов

Построен по `docs/tool-catalog.md` (файл сгенерирован из кода). Имя `bash` и имя `pwsh` встречаются дважды: обычный инструмент и вариант с постоянной оболочкой.

| Инструмент | Пакет | Параметры (`*` — обязательный) | Исходник |
|---|---|---|---|
| `plugin_manager` | `dsh-plugin-manager` | `action`*: string; `target`: string; `enabled`: boolean; `runtimeVersion`: string; `acceptRisk`: boolean; `approvedBuilds`: array[string]; `registry`: string; `offset`: number; `limit`: number | `packages/boot/plugin-manager/src/tools.ts` |
| `plugin_market` | `dsh-plugin-market` | `action`*: enum(list_indexes, list_plugins, list_versions); `target`: string; `name`: string; `query`: string; `offset`: number; `limit`: number | `packages/boot/plugin-market/src/tools.ts` |
| `list_mcp_resource_templates` | `dsh-mcp-resources` | `server`*: string; `cursor`: string | `packages/mcp/mcp-resources/src/tools.ts` |
| `list_mcp_resources` | `dsh-mcp-resources` | `server`*: string; `cursor`: string | `packages/mcp/mcp-resources/src/tools.ts` |
| `read_mcp_resource` | `dsh-mcp-resources` | `server`*: string; `uri`*: string | `packages/mcp/mcp-resources/src/tools.ts` |
| `stagehand_act` | `dsh-experimental-browser-use-stagehand-native` | `pageId`: string; `instruction`*: string | `packages/experimental/browser-use-stagehand-native/src/index.ts` |
| `stagehand_extract` | `dsh-experimental-browser-use-stagehand-native` | `pageId`: string; `instruction`*: string; `schema`: object | `packages/experimental/browser-use-stagehand-native/src/index.ts` |
| `stagehand_navigate` | `dsh-experimental-browser-use-stagehand-native` | `pageId`: string; `url`*: string | `packages/experimental/browser-use-stagehand-native/src/index.ts` |
| `stagehand_observe` | `dsh-experimental-browser-use-stagehand-native` | `pageId`: string; `instruction`*: string | `packages/experimental/browser-use-stagehand-native/src/index.ts` |
| `stagehand_screenshot` | `dsh-experimental-browser-use-stagehand-native` | `pageId`: string; `fullPage`*: boolean | `packages/experimental/browser-use-stagehand-native/src/index.ts` |
| `stagehand_tabs` | `dsh-experimental-browser-use-stagehand-native` | (нет параметров) | `packages/experimental/browser-use-stagehand-native/src/index.ts` |
| `propose_action` | `dsh-action-cards` | `action`*: string; `title`*: string; `brief`*: string; `details`*: object | `packages/experimental/action-cards/src/index.ts` |
| `ask_user_question` | `dsh-tool-ask-user` | `questions`*: array[object] | `packages/interaction/tool-ask-user/src/index.ts` |
| `run_code` | `dsh-tool-executor-ptc` | `code`*: string; `description`*: string; `timeoutMs`: number; `sandbox_permissions`: enum(workspace-write, danger-full-access); `justification`: string | `packages/core/tool-executor-ptc/src/ptc.ts` |
| `exit_plan_mode` | `dsh-plan-mode` | `plan`*: string | `packages/plan/plan-mode/src/index.ts` |
| `bash` | `dsh-tool-bash` | `description`*: string; `command`*: string; `timeoutMs`: number; `workdir`: string; `run_in_background`: boolean | `packages/shell/tool-bash/src/index.ts` |
| `office_convert` | `dsh-tool-office` | `input`*: string; `format`*: string; `sheet`: string | `packages/document/tool-office/src/index.ts` |
| `office_recalculate` | `dsh-tool-office` | `input`*: string; `format`*: enum(xlsx, ods) | `packages/document/tool-office/src/index.ts` |
| `office_render` | `dsh-tool-office` | `input`*: string; `pages`: array[integer]; `sheet`: string; `range`: string; `dpi`: integer | `packages/document/tool-office/src/index.ts` |
| `present` | `dsh-tool-present` | `files`*: array[object] | `packages/deliverables/tool-present/src/index.ts` |
| `pwsh` | `dsh-tool-pwsh` | `description`*: string; `command`*: string; `timeoutMs`: number; `workdir`: string; `run_in_background`: boolean | `packages/shell/tool-pwsh/src/index.ts` |
| `cordis_inspect_list` | `dsh-tool-cordis` | (нет параметров) | `packages/extensions/tool-cordis/src/index.ts` |
| `cordis_inspect_query` | `dsh-tool-cordis` | `platform`*: enum(host, client); `provider`*: string; `method`*: string; `input`: any | `packages/extensions/tool-cordis/src/index.ts` |
| `bash` | `dsh-tool-bash-persistent` | `command`*: string | `packages/shell/tool-bash-persistent/src/index.ts` |
| `pwsh` | `dsh-tool-pwsh-persistent` | `command`*: string | `packages/shell/tool-pwsh-persistent/src/index.ts` |
| `str_replace_editor` | `dsh-tool-str-replace-editor` | `command`*: enum(view, create, str_replace, insert); `path`*: string; `file_text`: oneOf; `insert_line`: oneOf; `new_str`: oneOf; `old_str`: oneOf; `view_range`: oneOf | `packages/fs/tool-str-replace-editor/src/index.ts` |
| `apply_patch` | `dsh-tool-fs` | `input`*: string | `packages/fs/tool-fs/src/index.ts` |
| `edit` | `dsh-tool-fs` | `file_path`*: string; `old_string`*: string; `new_string`*: string; `replace_all`: boolean | `packages/fs/tool-fs/src/index.ts` |
| `read` | `dsh-tool-fs` | `file_path`*: string; `offset`: number; `limit`: number | `packages/fs/tool-fs/src/index.ts` |
| `read_image` | `dsh-tool-fs` | `file_path`*: string | `packages/fs/tool-fs/src/index.ts` |
| `write` | `dsh-tool-fs` | `file_path`*: string; `content`*: string | `packages/fs/tool-fs/src/index.ts` |
| `glob` | `dsh-tool-fs-search` | `pattern`*: string; `path`: string | `packages/fs/tool-fs-search/src/index.ts` |
| `grep` | `dsh-tool-fs-search` | `pattern`*: string; `path`: string; `include`: string | `packages/fs/tool-fs-search/src/index.ts` |
| `terminal_close` | `dsh-tool-terminal` | `sessionId`*: string | `packages/experimental/tool-terminal/src/index.ts` |
| `terminal_list` | `dsh-tool-terminal` | (нет параметров) | `packages/experimental/tool-terminal/src/index.ts` |
| `terminal_open` | `dsh-tool-terminal` | `type`*: string; `name`: string; `cwd`: string | `packages/experimental/tool-terminal/src/index.ts` |
| `terminal_read` | `dsh-tool-terminal` | `sessionId`*: string; `offset`: number; `count`: number | `packages/experimental/tool-terminal/src/index.ts` |
| `terminal_send` | `dsh-tool-terminal` | `sessionId`*: string; `text`*: string; `submit`: boolean; `run_in_background`: boolean | `packages/experimental/tool-terminal/src/index.ts` |
| `terminal_signal` | `dsh-tool-terminal` | `sessionId`*: string; `signal`*: enum(SIGINT, SIGTERM, SIGKILL, SIGTSTP, SIGHUP) | `packages/experimental/tool-terminal/src/index.ts` |
| `user_terminal_list` | `dsh-terminal-panel` | (нет параметров) | `packages/experimental/terminal-panel/src/tools.ts` |
| `user_terminal_read` | `dsh-terminal-panel` | `id`*: string; `lines`: integer | `packages/experimental/terminal-panel/src/tools.ts` |
| `create_goal` | `dsh-tool-goal` | `objective`*: string; `max_goal_rounds`: number | `packages/goal/tool-goal/src/index.ts` |
| `get_goal` | `dsh-tool-goal` | (нет параметров) | `packages/goal/tool-goal/src/index.ts` |
| `update_goal` | `dsh-tool-goal` | `goal_id`*: string; `revision`*: number; `action`*: enum(edit, pause, resume, complete, blocked); `objective`: string; `max_goal_rounds`: number; `blocked_reason`: string | `packages/goal/tool-goal/src/index.ts` |
| `schedule_create` | `dsh-tool-schedule` | `title`*: string; `prompt`*: string; `rrule`*: string; `target`*: enum(this_session, new_once, new) | `packages/schedule/tool-schedule/src/index.ts` |
| `schedule_edit` | `dsh-tool-schedule` | `id`*: string; `action`*: enum(update, delete); `title`: string; `prompt`: string; `rrule`: string; `target`: enum(this_session, new_once, new); `active`: boolean; `expectedUpdatedAt`: string | `packages/schedule/tool-schedule/src/index.ts` |
| `schedule_list` | `dsh-tool-schedule` | `active`: boolean; `limit`: integer; `offset`: integer | `packages/schedule/tool-schedule/src/index.ts` |
| `schedule_view` | `dsh-tool-schedule` | `id`*: string | `packages/schedule/tool-schedule/src/index.ts` |
| `lsp` | `dsh-tool-lsp` | `operation`*: enum(goToDefinition, findReferences, goToImplementation, hover); `file_path`*: string; `line`*: number; `character`*: number | `packages/lsp/tool-lsp/src/index.ts` |
| `ralph` | `dsh-tool-ralph` | `objective`*: string; `maxRounds`: number | `packages/experimental/tool-ralph/src/index.ts` |
| `skill` | `dsh-tool-skill` | `name`*: string | `packages/skill/tool-skill/src/index.ts` |
| `session_event_read` | `dsh-tool-session-query` | `session_id`: string; `seq`*: integer; `before`: integer; `after`: integer | `packages/experimental/tool-session-query/src/index.ts` |
| `session_event_search` | `dsh-tool-session-query` | `session_id`: string; `query`*: string; `seq_from`: integer; `seq_to`: integer; `time_from`: string; `time_to`: string; `event_types`: array[string]; `surfaces`: array[string] | `packages/experimental/tool-session-query/src/index.ts` |
| `session_event_trace` | `dsh-tool-session-query` | `session_id`: string; `seq`*: integer | `packages/experimental/tool-session-query/src/index.ts` |
| `session_search` | `dsh-tool-session-query` | `query`*: string; `session_ids`: array[string]; `created_at_from`: string; `created_at_to`: string; `parent_session_ids`: array[string]; `include_root_sessions`: boolean; `availability`: array[string]; `event_seq_from`: integer; `event_seq_to`: integer; `event_time_from`: string; `event_time_to`: string; `event_types`: array[string]; `event_surfaces`: array[string] | `packages/experimental/tool-session-query/src/index.ts` |
| `session_trace` | `dsh-tool-session-query` | `session_id`: string | `packages/experimental/tool-session-query/src/index.ts` |
| `subagent_oar` | `dsh-experimental-tool-subagent-oar` | `runtime`*: enum(codex, claude); `description`*: string; `name`: string; `prompt`*: string; `cwd`: string | `packages/experimental/tool-subagent-oar/src/index.ts` |
| `list_subagent_models` | `dsh-tool-subagent` | `provider`: string; `model`: string | `packages/subagent/tool-subagent/src/list-models.ts` |
| `subagent` | `dsh-tool-subagent` | `cwd`: string; `description`*: string; `name`: string; `prompt`*: string | `packages/subagent/tool-subagent/src/index.ts` |
| `interrupt_agent` | `dsh-tool-subagent-control` | `agent`*: string | `packages/subagent/tool-subagent-control/src/index.ts` |
| `list_agents` | `dsh-tool-subagent-control` | (нет параметров) | `packages/subagent/tool-subagent-control/src/list-agents.ts` |
| `send_message` | `dsh-tool-subagent-control` | `agent`*: string; `message`*: string | `packages/subagent/tool-subagent-control/src/index.ts` |
| `wait_agent` | `dsh-tool-subagent-control` | `timeout_ms`: integer | `packages/subagent/tool-subagent-control/src/wait-agent.ts` |
| `job_kill` | `dsh-tool-jobs` | `job_id`*: string; `reason`: string | `packages/jobs/tool-jobs/src/index.ts` |
| `job_list` | `dsh-tool-jobs` | (нет параметров) | `packages/jobs/tool-jobs/src/index.ts` |
| `job_output` | `dsh-tool-jobs` | `job_id`*: string; `wait`: boolean; `timeout_ms`: number | `packages/jobs/tool-jobs/src/index.ts` |
| `team_task_create` | `dsh-tool-agent-team` | `subject`*: string; `description`*: string; `blocked_by`: array[string]; `write_scopes`: array[string] | `packages/team/tool-agent-team/src/index.ts` |
| `team_task_get` | `dsh-tool-agent-team` | `task_id`*: string | `packages/team/tool-agent-team/src/index.ts` |
| `team_task_list` | `dsh-tool-agent-team` | `status`: enum(pending, in_progress, completed); `owner`: string; `unowned`: boolean; `ready`: boolean; `cursor`: integer; `limit`: integer | `packages/team/tool-agent-team/src/index.ts` |
| `team_task_update` | `dsh-tool-agent-team` | `task_id`*: string; `expected_revision`*: integer; `action`*: string; `subject`: string; `description`: string; `blocked_by`: array[string]; `write_scopes`: array[string]; `owner`: string | `packages/team/tool-agent-team/src/index.ts` |
| `todo_write` | `dsh-tool-todo` | `todos`*: array[object] | `packages/todo/tool-todo/src/index.ts` |
| `working_directory` | `dsh-tool-working-directory` | `cd`: string | `packages/session/tool-working-directory/src/index.ts` |
| `create_worktree` | `dsh-experimental-tool-worktree` | `name`: string; `from`: string | `packages/experimental/tool-worktree/src/index.ts` |
| `workflow` | `dsh-tool-workflow` | `script`*: string; `meta`*: object; `args`: object; `run_in_background`: boolean | `packages/workflow/tool-workflow/src/index.ts` |
| `load_workspace_dependencies` | `dsh-tool-workspace-dependencies` | (нет параметров) | `packages/skill/tool-workspace-dependencies/src/index.ts` |
| `web_fetch` | `dsh-tool-web` | `url`*: string | `packages/web/tool-web/src/index.ts` |
| `web_search` | `dsh-tool-web` | `queries`*: array[string] | `packages/web/tool-web/src/index.ts` |
