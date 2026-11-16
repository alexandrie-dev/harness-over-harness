# Ядро HoH: `hoh-kernel`, `hoh-kernel-macros`, `hoh-loader`

Спецификация трёх crates этапа 0. По ней пишется ядро и против неё переносятся пакеты dsh.
Документ составлен по чтению источников; ничего из образца не запускалось.

Обозначения источников:

- `[U]` — upstream Cordis, `/mnt/toshiba/alexandrie/hoh/refs/cordis/packages/core` (коммит `c9fe179`, 4.0.0-rc.13).
  `fiber.ts`, `events.ts`, `reflect.ts`, `logger.ts` без пометки — файлы `[U] src/`. Тест записан как
  `fiber.spec «имя случая»` и лежит в `[U] tests/`.
- `[D]` — dsh, `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness` (коммит `39163dd3`): `vendor/*`, `scripts/*.spec.ts`,
  `packages/boot/*`. `V6` и подобные — записи журнала локальных правок `[D] vendor/README.md`.
- «Статья» — arXiv:2608.25512, §5.2.1 (перекомпоновка), §6.4 (другие языки).

Числа использования в dsh посчитаны по `packages/*/src` без `client`, `experimental` и тестов.

## 1. Понятия

**Kernel** — одна задача tokio, единственный владелец таблицы fiber, их состояний и таблицы сервисов. Любое
изменение жизненного цикла есть сообщение этой задаче. Код плагинов в ней не исполняется.

**Plugin** — тип с функцией `apply(ctx, deps, config)`. Код слинкован статически; состав задаётся данными.

**Fiber** — один запуск плагина с одной конфигурацией: состояние, владение эффектами, набор зависимостей.
Корневой fiber (uid 0) всегда активен и плагина не имеет. Дочерний fiber есть эффект родителя.

**Context** — дешёвое значение (`Clone`), через которое плагин делает всё: эффекты, сервисы, события, дочерние
плагины. Контекст принадлежит одной активации одного fiber и несёт таблицу областей, перехваты и метки.
Производные контексты (`isolate`, `intercept`, `with_tag`, `with_filter`) неизменяемы.

**Effect** — действие вместе с обратным действием (`Undo`). Эффект принадлежит fiber и снимается либо при
выгрузке fiber, либо раньше через `Disposer`. Обратное действие исполняется ровно один раз и может быть
асинхронным. `Drop` ничего не снимает, а только сообщает о потерянном эффекте.

**Service** — именованное значение, которое один fiber предоставляет, а другие получают. Ключ сервиса есть
тип (`ServiceKey`) со строковым именем и типом значения. Предоставление есть эффект.

**Realm** — область имён сервиса. Запись в таблице сервисов адресуется парой (имя, область). `isolate`
создаёт контекст, в котором одно имя смотрит в другую область.

**Inject** — объявленные обязательные зависимости плагина: структура с полями `Service<K>`. Fiber запускается,
когда все они доступны, выгружается, когда хотя бы одна пропала, и запускается снова, когда вернулась.
Набор значений, с которым fiber запущен (committed view), остаётся его до конца выгрузки, а поставщик ждёт
своих потребителей.

**Event** — тип-сообщение с именем и режимом рассылки (один из пяти). Слушатель есть эффект.

**Config** — тип конфигурации плагина: serde + JSON Schema. Сырое значение (JSON) хранится в fiber, плагин
получает разобранное. Обновление доходит до работающего плагина одним из трёх путей: перезапуск, замена
снимка на месте (изменились только поля `Volatile`), собственный обработчик.

**Loader** — превращает строки конфигурации (`Entry`) в fiber: имя плагина ищется в таблице «имя →
конструктор», строки объединяются в группы (`Group`), состав получается наложением слоёв патчей (`Patch`)
профиля (`Profile`). При изменении состава для каждой строки выбирается наименее разрушительное действие.

## 2. Публичный API

Сигнатуры обязательны; приватные поля и вспомогательные типы на усмотрение исполнителя. Все операции ядра,
возвращающие `impl Future`, **отправляют команду в момент вызова**: future только ждёт исход, и его можно
не ждать и уронить (правило R59).

### 2.1. `hoh-kernel`: ядро, fiber, плагин

```rust
pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum KernelError {
    #[error("inactive context")]
    Inactive,                                   // Cordis INACTIVE_EFFECT
    #[error("service \"{name}\" has been registered at <{owner}>")]
    DuplicateService { name: String, owner: String },
    #[error("service \"{name}\" is declared with another value type")]
    ServiceType { name: String },
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("one or more listeners of event \"{event}\" failed")]
    Listeners { event: &'static str, errors: Vec<std::sync::Arc<Error>> },
    #[error("{0}")]
    Failed(std::sync::Arc<Error>),              // ошибка активации, общая для всех ожидающих
    #[error("config type does not match the plugin's Config")]
    ConfigType,                                 // `on_update::<C>` или `on_candidate::<C>` с чужим `C`
    #[error("a fiber cannot wait for its own transition")]
    SelfWait,
    #[error("kernel has shut down")]
    Closed,
}

#[derive(Clone)]
pub struct Kernel { /* .. */ }
impl Kernel {
    /// Порождает задачу ядра в текущем runtime tokio.
    pub fn start() -> Kernel;
    pub fn root(&self) -> Context;
    pub fn snapshot(&self) -> impl Future<Output = Snapshot> + Send + 'static;
    /// Снимает все эффекты корня (то есть всё дерево) и останавливает задачу ядра.
    pub fn shutdown(&self) -> impl Future<Output = ()> + Send + 'static;
}

pub struct Snapshot { pub fibers: Vec<FiberInfo>, pub provisions: Vec<ProvisionInfo> }
pub struct FiberInfo {
    pub uid: u32, pub parent: u32, pub name: Arc<str>, pub entry_id: Option<Arc<str>>,
    pub state: FiberState, pub missing: Vec<Arc<str>>, pub error: Option<String>,
}
pub struct ProvisionInfo {
    pub name: Arc<str>, pub realm: Realm, pub owner: u32, pub active: bool, pub dependents: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState { Pending, Loading, Active, Failed, Disposed, Unloading }

#[derive(Clone)]
pub struct Fiber { /* uid + ссылка на ядро */ }
impl Fiber {
    pub fn uid(&self) -> u32;
    pub fn name(&self) -> Arc<str>;                 // Plugin::NAME; иначе имя ближайшего предка; иначе "root"
    pub fn entry_id(&self) -> Option<Arc<str>>;     // id строки конфигурации, если fiber создан загрузчиком
    pub fn state(&self) -> FiberState;
    pub fn is_disposed(&self) -> bool;
    pub fn raw_config(&self) -> Arc<serde_json::Value>;
    pub fn effective_config(&self) -> Option<Arc<serde_json::Value>>;
    pub fn resolve_config(&self, candidate: &serde_json::Value) -> Result<serde_json::Value, KernelError>;
    pub fn settled(&self) -> impl Future<Output = Result<(), KernelError>> + Send + 'static;
    pub fn update(&self, raw: serde_json::Value) -> impl Future<Output = Result<(), KernelError>> + Send + 'static;
    pub fn commit_volatile(&self, raw: serde_json::Value)
        -> impl Future<Output = Result<bool, KernelError>> + Send + 'static;
    pub fn restart(&self) -> impl Future<Output = Result<(), KernelError>> + Send + 'static;
    pub fn dispose(&self) -> impl Future<Output = ()> + Send + 'static;
}

pub trait Plugin: Send + Sync + 'static {
    /// Имя fiber и журнала (в dsh: `export const name`). Не имя строки конфигурации.
    const NAME: &'static str;
    type Config: Config;
    type Deps: Inject;
    fn apply(ctx: Context, deps: Self::Deps, config: Cfg<Self::Config>)
        -> impl Future<Output = Result<(), Error>> + Send;
}

/// Стёртое описание плагина для загрузчика.
#[derive(Clone)]
pub struct PluginDef { /* .. */ }
impl PluginDef {
    pub fn of<P: Plugin>() -> PluginDef;
    pub fn name(&self) -> &'static str;
    pub fn schema(&self) -> &serde_json::Value;     // JSON Schema 2020-12 типа P::Config
    pub fn deps(&self) -> &[Dep];
}

#[derive(Default)]
pub struct PluginOptions {
    pub entry_id: Option<Arc<str>>,
    pub inject: Vec<Arc<str>>,                      // дополнительные обязательные сервисы по имени (строка `inject`)
    pub resolver: Option<Arc<dyn ConfigResolver>>,
}
pub trait ConfigResolver: Send + Sync + 'static {
    /// Сырой config → config без выражений. Вызывается перед проверкой схемы. Не блокирует.
    fn resolve(&self, ctx: &Context, raw: &serde_json::Value) -> Result<serde_json::Value, Error>;
}
```

### 2.2. Контекст

```rust
#[derive(Clone)]
pub struct Context { /* fiber, поколение активации, области, перехваты, метки, фильтр */ }

impl Context {
    pub fn fiber(&self) -> Fiber;
    pub fn root(&self) -> Context;
    pub fn ensure_active(&self) -> Result<(), KernelError>;       // Fiber.assertActive
    pub fn logger(&self) -> Logger;
    /// Отменяется в момент, когда эта активация перестаёт быть текущей (начало выгрузки или замещение).
    pub fn token(&self) -> tokio_util::sync::CancellationToken;

    // --- дочерние fiber
    pub fn plugin<P: Plugin>(&self, raw: serde_json::Value) -> Result<Fiber, KernelError>;
    pub fn plugin_dyn(&self, def: &PluginDef, raw: serde_json::Value, options: PluginOptions)
        -> Result<Fiber, KernelError>;
    /// `ctx.inject([...], callback)`: безымянный дочерний fiber с зависимостями `D`.
    pub fn inject<D, Fut>(&self, body: impl Fn(Context, D) -> Fut + Send + Sync + 'static)
        -> Result<Fiber, KernelError>
    where D: Inject, Fut: Future<Output = Result<(), Error>> + Send + 'static;

    // --- эффекты
    pub fn effect(&self, label: &'static str, setup: impl FnOnce() -> Undo) -> Result<Disposer, KernelError>;
    pub fn try_effect<E: From<KernelError>>(
        &self, label: &'static str, setup: impl FnOnce(&Undos) -> Result<(), E>,
    ) -> Result<Disposer, E>;
    pub async fn effect_async<E, Fut>(
        &self, label: &'static str, setup: impl FnOnce(Undos) -> Fut,
    ) -> Result<Disposer, E>
    where E: From<KernelError>, Fut: Future<Output = Result<(), E>> + Send;
    /// Фоновая задача как эффект: снятие отменяет токен и ждёт завершения задачи.
    pub fn spawn<Fut>(
        &self, label: &'static str,
        task: impl FnOnce(tokio_util::sync::CancellationToken) -> Fut + Send + 'static,
    ) -> Result<Disposer, KernelError>
    where Fut: Future<Output = Result<(), Error>> + Send + 'static;

    // --- сервисы и области
    pub fn provide<K: ServiceKey>(&self, value: Arc<K::Value>)
        -> impl Future<Output = Result<Disposer, KernelError>> + Send + 'static;
    pub fn get<K: ServiceKey>(&self) -> Option<Service<K>>;
    pub fn has(&self, service: &str) -> bool;
    pub fn view(&self, service: &str) -> Option<serde_json::Value>;   // ServiceKey::view активного поставщика
    pub fn realm_of(&self, service: &str) -> Realm;
    pub fn isolate(&self, service: &str, realm: Realm) -> Context;
    pub fn intercept(&self, service: &str, options: serde_json::Value) -> Context;
    pub fn intercepted<T: serde::de::DeserializeOwned + Default>(&self, service: &str) -> T;

    // --- метки и фильтр рассылки
    pub fn with_tag<T: Send + Sync + 'static>(&self, tag: T) -> Context;
    pub fn tag<T: Send + Sync + 'static>(&self) -> Option<Arc<T>>;   // ближайшая метка этого типа
    pub fn with_filter(&self, filter: impl Fn(&Context) -> bool + Send + Sync + 'static) -> Context;

    // --- обновление конфигурации на месте (раздел 3.6)
    pub fn on_candidate<C: Config>(&self, check: impl Fn(&C) -> Result<(), Error> + Send + Sync + 'static)
        -> Result<Disposer, KernelError>;
    pub fn on_update<C, Fut>(&self, handler: impl Fn(Arc<C>, Next<()>) -> Fut + Send + Sync + 'static)
        -> Result<Disposer, KernelError>
    where C: Config, Fut: Future<Output = Result<(), Error>> + Send + 'static;

    // --- время
    pub fn sleep(&self, duration: std::time::Duration)
        -> impl Future<Output = Result<(), KernelError>> + Send + 'static;
    pub fn interval(&self, period: std::time::Duration) -> impl futures_util::Stream<Item = ()> + Send + 'static;
}
```

### 2.3. Эффекты

```rust
#[must_use = "setup эффекта обязан вернуть обратное действие"]
pub struct Undo { /* .. */ }
impl Undo {
    pub fn none() -> Undo;
    pub fn sync(f: impl FnOnce() + Send + 'static) -> Undo;
    pub fn future<Fut>(f: impl FnOnce() -> Fut + Send + 'static) -> Undo
    where Fut: Future<Output = Result<(), Error>> + Send + 'static;
}
/// Уже зарегистрированный эффект переходит во владение составного (в Cordis: `yield ctx.on(...)`).
impl From<Disposer> for Undo { /* .. */ }

/// Сборщик обратных действий составного эффекта.
#[derive(Clone)]
pub struct Undos { /* .. */ }
impl Undos {
    pub fn push(&self, undo: impl Into<Undo>) -> Result<(), KernelError>;
}

/// Право снять эффект раньше выгрузки fiber. Не `Clone`. Уронить можно: эффект остаётся у fiber.
pub struct Disposer { /* .. */ }
impl Disposer {
    pub fn dispose(self) -> impl Future<Output = ()> + Send + 'static;
}
```

### 2.4. Сервисы и зависимости

```rust
pub trait ServiceKey: Send + Sync + 'static {
    type Value: ?Sized + Send + Sync + 'static;
    const NAME: &'static str;                       // имя сервиса dsh без изменений: "tools", "headlessStartup"
    /// Данные сервиса для выражений конфигурации (`ctx.<имя>.<поле>`). По умолчанию нет.
    fn view(_value: &Self::Value) -> Option<serde_json::Value> { None }
}

/// service!(pub Tools: dyn ToolsApi = "tools");  →  pub struct Tools; impl ServiceKey for Tools { .. }
#[macro_export] macro_rules! service { /* .. */ }

/// Ссылка на значение сервиса. Публичного конструктора нет: получить можно только из `Deps` или `ctx.get`.
pub struct Service<K: ServiceKey> { /* Arc<K::Value> */ }
impl<K: ServiceKey> Clone for Service<K> { /* .. */ }
impl<K: ServiceKey> std::ops::Deref for Service<K> { type Target = K::Value; /* .. */ }

pub struct Dep { pub name: &'static str, pub key: std::any::TypeId }

/// Реализуется только через `#[derive(Inject)]` и для `()`.
pub trait Inject: Sized + Send + 'static {
    fn deps() -> Vec<Dep>;
    #[doc(hidden)] fn resolve(view: &View) -> Self;
}
#[doc(hidden)] pub struct View { /* committed view */ }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Realm(u64);
impl Realm {
    pub const ROOT: Realm = Realm(0);
    pub fn new() -> Realm;                          // новая область, уникальная в процессе
}
```

### 2.5. События

```rust
pub trait Event: Send + Sync + 'static {
    const NAME: &'static str;                       // имя события dsh без изменений: "tools/result"
    type Mode: Mode;
}
pub trait Mode: 'static { const DISPATCH: Dispatch; }   // запечатан, реализован пятью типами ниже
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch { Emit, Parallel, Serial, Bail, Waterfall }

pub struct Emit;
pub struct Parallel;
pub struct Serial<R>(std::marker::PhantomData<fn() -> R>);
pub struct Bail<R>(std::marker::PhantomData<fn() -> R>);
pub struct Waterfall<R>(std::marker::PhantomData<fn() -> R>);

/// event!(ToolsResult = "tools/result", Emit);  →  impl Event for ToolsResult { .. }
#[macro_export] macro_rules! event { /* .. */ }

/// Продолжение цепочки waterfall. Вызов потребляет значение, поэтому второй вызов не компилируется.
pub struct Next<R> { /* .. */ }
impl<R: Send + 'static> Next<R> {
    pub fn run(self) -> impl Future<Output = Result<R, Error>> + Send + 'static;
}

pub struct Listen<'a> { /* .. */ }
impl<'a> Listen<'a> {
    pub fn prepend(self) -> Self;                   // слушатель встаёт первым
    pub fn global(self) -> Self;                    // слушатель не подчиняется фильтру рассылки
    pub fn on<E>(self, f: impl Fn(&E) + Send + Sync + 'static) -> Result<Disposer, KernelError>
    where E: Event<Mode = Emit>;
    pub fn on_bail<E, R>(self, f: impl Fn(&E) -> Option<R> + Send + Sync + 'static) -> Result<Disposer, KernelError>
    where E: Event<Mode = Bail<R>>, R: Send + 'static;
    pub fn on_parallel<E, Fut>(self, f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static) -> Result<Disposer, KernelError>
    where E: Event<Mode = Parallel>, Fut: Future<Output = Result<(), Error>> + Send + 'static;
    pub fn on_serial<E, R, Fut>(self, f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static) -> Result<Disposer, KernelError>
    where E: Event<Mode = Serial<R>>, R: Send + 'static, Fut: Future<Output = Result<Option<R>, Error>> + Send + 'static;
    pub fn on_waterfall<E, R, Fut>(self, f: impl Fn(Arc<E>, Next<R>) -> Fut + Send + Sync + 'static)
        -> Result<Disposer, KernelError>
    where E: Event<Mode = Waterfall<R>>, R: Send + 'static, Fut: Future<Output = Result<R, Error>> + Send + 'static;
}

impl Context {
    pub fn listen(&self) -> Listen<'_>;
    // on, on_bail, on_parallel, on_serial, on_waterfall: то же, что self.listen().on*(..)

    pub fn emit<E: Event<Mode = Emit>>(&self, event: &E);
    pub fn bail<E, R>(&self, event: &E) -> Option<R> where E: Event<Mode = Bail<R>>, R: Send + 'static;
    pub fn parallel<E: Event<Mode = Parallel>>(&self, event: impl Into<Arc<E>>)
        -> impl Future<Output = Result<(), KernelError>> + Send + 'static;
    pub fn serial<E, R>(&self, event: impl Into<Arc<E>>)
        -> impl Future<Output = Result<Option<R>, Error>> + Send + 'static
    where E: Event<Mode = Serial<R>>, R: Send + 'static;
    pub fn waterfall<E, R, Fut>(&self, event: impl Into<Arc<E>>, tail: impl FnOnce(Arc<E>) -> Fut + Send + 'static)
        -> impl Future<Output = Result<R, Error>> + Send + 'static
    where E: Event<Mode = Waterfall<R>>, R: Send + 'static, Fut: Future<Output = Result<R, Error>> + Send + 'static;
}

// События жизненного цикла. Все в режиме Emit, рассылаются задачей ядра.
pub struct InternalPlugin { pub fiber: Fiber }                          // "internal/plugin": создание и снятие
pub struct InternalStatus { pub fiber: Fiber, pub old: FiberState }     // "internal/status"
pub struct InternalService { pub name: Arc<str>, pub realm: Realm, pub available: bool }   // "internal/service"
pub struct VolatileUpdate { pub paths: Vec<String> }                    // "loader/volatile-update"

impl Context {
    /// "internal/dispatch": наблюдатель всех рассылок, кроме событий ядра. Полезная нагрузка заимствована,
    /// поэтому это отдельный метод, а не `Event`.
    pub fn on_dispatch(
        &self, f: impl Fn(Dispatch, &'static str, &(dyn std::any::Any + Send + Sync)) + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>;
}
```

### 2.6. Конфигурация

```rust
pub trait Config: serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema + Send + Sync + 'static {}
impl<T> Config for T where T: serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema + Send + Sync + 'static {}

/// Плагин без настроек: принимает `null`, отсутствие значения и любой объект.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct NoConfig {}

/// Живая ссылка на конфигурацию экземпляра.
pub struct Cfg<C: Config> { /* .. */ }
impl<C: Config> Clone for Cfg<C> { /* .. */ }
impl<C: Config> Cfg<C> {
    pub fn get(&self) -> Arc<C>;                    // снимок последнего принятого обновления
}

/// Поле, изменение которого не требует перезапуска (в dsh: `.volatile()`).
/// serde: прозрачно. JSON Schema: схема `T` плюс `"x-volatile": true`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Volatile<T>(pub T);
impl<T> std::ops::Deref for Volatile<T> { type Target = T; /* .. */ }

#[derive(Clone, Debug, PartialEq)]
pub enum PathSeg { Key(String), Index(usize) }
#[derive(Clone, Debug, PartialEq)]
pub struct Issue { pub message: String, pub path: Vec<PathSeg> }
#[derive(Clone, Debug, PartialEq)]
pub struct ValidationError { pub issues: Vec<Issue> }   // Display: правило R47

pub mod config {
    pub fn validate(schema: &Value, value: &Value) -> Result<(), ValidationError>;
    pub fn equal_except_volatile(schema: &Value, a: &Value, b: &Value) -> bool;
    pub fn volatile_paths(schema: &Value, a: &Value, b: &Value) -> Vec<String>;
}
```

### 2.7. Журнал

Перенос `[D] vendor/cordis/src/logger.ts` и `vendor/logger-console`. Журнал принадлежит экземпляру ядра, поэтому
тесты читают его без глобального состояния.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level { Error = 0, Warn = 1, Info = 2, Debug = 3 }

#[derive(Clone, Debug)]
pub struct Record { pub sn: u64, pub ts_ms: i64, pub name: Arc<str>, pub level: Level, pub message: String, pub fiber: u32 }

#[derive(Clone, Debug, Default)]
pub struct Levels { pub default: Option<Level>, pub by_name: std::collections::HashMap<String, Level> }

#[derive(Clone)]
pub struct Logger { /* .. */ }
impl Logger {
    pub fn named(&self, name: &str) -> Logger;                  // ctx.logger('name')
    pub fn error(&self, message: impl std::fmt::Display);
    pub fn warn(&self, message: impl std::fmt::Display);
    pub fn info(&self, message: impl std::fmt::Display);
    pub fn debug(&self, message: impl std::fmt::Display);
    /// Приёмник записей как эффект вызывающего fiber.
    pub fn exporter(&self, levels: Levels, export: impl Fn(&Record) + Send + Sync + 'static)
        -> Result<Disposer, KernelError>;
    pub fn buffer(&self) -> Vec<Record>;                        // последние 1000 записей по порядку
}
/// Плагин "logger-console": пишет записи в stderr. Config: { levels, showTime, showDiff } как в образце.
pub struct ConsoleExporter;
```

### 2.8. `hoh-kernel-macros`

```rust
/// Только структура с именованными полями типа `Service<K>`. Порождает `impl Inject`.
#[derive(Inject)]
pub struct Deps { tools: Service<Tools>, llm: Service<Llm> }

/// Сокращение для плагина-функции. Порождает `pub struct GreetTool;` и `impl Plugin for GreetTool`.
/// Параметры узнаются по имени: `ctx` обязателен; `deps` и `config` можно опустить
/// (тогда `Deps = ()`, `Config = NoConfig`). Ведущий `_` в имени допустим. Тип `config` обязан быть `Cfg<C>`.
#[plugin(GreetTool, name = "greet-tool")]
pub async fn apply(ctx: Context, deps: Deps, config: Cfg<GreetConfig>) -> Result<(), Error> { /* .. */ }
```

Плагин можно объявить и явным `impl Plugin for X`; макрос ничего сверх него не делает. Ошибки компиляции,
которые макросы обязаны давать с понятным текстом: поле `Deps` не типа `Service<K>`; параметр с другим именем;
`config` не `Cfg<_>`; функция не `async`.

### 2.9. `hoh-loader`

```rust
/// Строка конфигурации (dsh `EntryOptions`). Ключи YAML совпадают с именами полей.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    #[serde(default)] pub id: String,                               // пустой заменяется сгенерированным
    pub name: String,
    #[serde(default, skip_serializing_if = "Value::is_null")] pub config: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")] pub disabled: Value,   // bool, null или выражение
    #[serde(default, skip_serializing_if = "is_false")] pub group: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub inject: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub isolate: Option<IndexMap<String, Isolate>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub intercept: Option<IndexMap<String, Value>>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Isolate { Private, Label(String) }                         // YAML: `true` | "метка"

/// Элемент патча (dsh `PatchOptions`).
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub id: Option<String>,
    pub name: Option<String>,                                       // утверждение об имени цели
    pub set: IndexMap<String, Value>,                               // остальные ключи: замена целиком
    pub insert: Option<Vec<Entry>>,
}

#[derive(Clone, Debug)]
pub enum LayerSource { Bundle(String), Profile, Home, CommandLine(std::path::PathBuf), Launcher(&'static str) }
#[derive(Clone, Debug)]
pub struct Layer { pub source: LayerSource, pub file: Option<std::path::PathBuf>, pub patches: Vec<Patch> }

pub struct Profile {
    pub name: String,
    pub dir: std::path::PathBuf,
    pub layers: Vec<Layer>,                                         // в порядке применения, см. L9
    pub skipped: Vec<SkippedBundle>,
}
pub struct SkippedBundle { pub name: String, pub reason: String }
pub struct Bundle { pub name: String, pub dir: Option<std::path::PathBuf>, pub platforms: Option<Vec<String>>, pub files: Vec<(String, String)> }
pub trait BundleSource: Send + Sync { fn bundle(&self, name: &str) -> Result<Bundle, LoadError>; }

impl Profile {
    pub fn load(dir: &std::path::Path, bundles: &dyn BundleSource, home: &std::path::Path,
                overlays: &[std::path::PathBuf], env: &EvalEnv) -> Result<Profile, LoadError>;
}

/// Чистые функции состава: одни и те же для запуска, перекомпоновки и `--dump-config` (V11).
pub fn parse_entries(text: &str, file: &std::path::Path) -> Result<Vec<Entry>, LoadError>;
pub fn parse_patches(text: &str, file: &std::path::Path) -> Result<Vec<Patch>, LoadError>;
pub fn apply_patches(base: &[Entry], patches: &[Patch], warn: &mut dyn FnMut(String)) -> Vec<Entry>;
pub fn compose(base: &[Entry], layers: &[Layer], warn: &mut dyn FnMut(String)) -> Composition;
pub fn render_dump(composition: &Composition) -> String;
pub struct Composition { pub entries: Vec<Entry>, pub origins: IndexMap<String, Origin> }
pub struct Origin { pub created_by: String, pub patched_by: Vec<String> }

/// Таблица «имя → конструктор».
#[derive(Default)]
pub struct PluginRegistry { /* .. */ }
impl PluginRegistry {
    pub fn add<P: Plugin>(&mut self, name: &str) -> &mut Self;      // имя HoH: "hoh-tools", "hoh-plugin-manager/tools"
    pub fn alias(&mut self, alias: &str, name: &str) -> &mut Self;  // "@deepseek-ai/dsh-tools" → "hoh-tools"
    pub fn carrier(&mut self, name: &str) -> &mut Self;             // плагин сам монтирует строки из своего config (L7)
    pub fn resource_dir(&mut self, name: &str, dir: std::path::PathBuf) -> &mut Self;
    pub fn resolve(&self, name: &str) -> Option<&PluginDef>;
}

/// Среда вычисления выражений. В тестах задаётся целиком, без обращения к настоящему окружению.
#[derive(Clone)]
pub struct EvalEnv {
    pub home: std::path::PathBuf,           // для dshHomePath(..)
    pub cwd: std::path::PathBuf,
    pub exe: std::path::PathBuf,
    pub platform: &'static str,             // "linux" | "darwin" | "win32"
    pub var: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

pub struct BootOptions {
    pub registry: Arc<PluginRegistry>,
    pub base: std::path::PathBuf,           // базовый файл строк приложения
    pub layers: Vec<Layer>,
    pub env: EvalEnv,
}
/// Предоставляет сервис "loader" в корне, монтирует корневое включение (id "include") и ждёт `settled`.
pub async fn boot(root: &Context, options: BootOptions) -> Result<Service<LoaderKey>, LoadError>;
service!(pub LoaderKey: Loader = "loader");

pub struct Loader { /* .. */ }
impl Loader {
    pub fn entries(&self) -> Vec<EntryInfo>;                        // все строки, включая вложенные, в порядке документа
    pub fn resolve(&self, id: &str) -> Option<EntryInfo>;
    pub async fn create(&self, entry: Entry, group: Option<&str>) -> Result<String, LoadError>;
    pub async fn remove(&self, id: &str) -> Result<(), LoadError>;
    pub async fn settled(&self);                                    // loader.await()
    pub async fn recompose(&self, layers: Vec<Layer>) -> Result<Vec<EntryChange>, LoadError>;
    pub fn audit(&self, required: &[&str]) -> Audit;
    pub fn dump(&self) -> String;
}
pub struct EntryInfo {
    pub id: String, pub name: String, pub group: Option<String>, pub disabled: bool,
    pub options: Entry, pub fiber: Option<Fiber>, pub status: EntryStatus,
}
pub enum EntryStatus { Disabled, Unresolved, Mounted }
pub enum EntryChange { Created(String), Updated(String, UpdateAction), Removed(String) }
pub enum UpdateAction { None, Volatile, Config, Rebuilt, Enabled, Disabled }
pub struct Audit { pub missing: Vec<String>, pub unresolved: Vec<String>, pub inactive: Vec<FiberInfo> }

/// Группа строк под произвольным контекстом (в dsh: собственное `EntryTree` у agent-preset-registry).
pub struct Group { /* .. */ }
impl Group {
    pub fn mount(ctx: &Context, registry: Arc<PluginRegistry>, env: EvalEnv) -> Result<Group, KernelError>;
    pub async fn update(&self, rows: Vec<Entry>) -> Vec<EntryChange>;
    pub fn entries(&self) -> Vec<EntryInfo>;
}

/// Выражение конфигурации (раздел 5.5).
pub enum Expr { /* закрытый набор термов */ }
impl Expr {
    pub fn parse(source: &str) -> Result<Expr, ExprError>;
    pub fn eval(&self, env: &EvalEnv, ctx: &Context, registry: &PluginRegistry, base_dir: &std::path::Path)
        -> Result<Option<Value>, ExprError>;                        // None = `undefined`
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError { /* Io, Yaml { file, line, column, message }, Shape, Expr { file, line, source, reason }, Kernel, .. */ }
```

Встроенные плагины загрузчика: `cordis:group` (она же `@deepseek-ai/cordis-plugin-group`) и `cordis:include`
(`@deepseek-ai/cordis-plugin-include`). Их `Config` — список строк и `{ path, patches }` соответственно.

## 3. Правила

### 3.1. Fiber и жизненный цикл

- **R1. Состояние.** `Disposed`, если fiber снят; иначе `Failed`, если хранится ошибка; иначе `Loading` или
  `Unloading`, пока идёт переход; иначе `Active`, если эпоха задана; иначе `Pending`. (`fiber.ts` `_getState`.)
- **R2. Эпоха.** Эпоха fiber — пара (ревизия, uid записи сервиса для каждого объявленного имени). Если хотя бы
  одно имя не разрешено, эпоха пуста. Ревизию увеличивают `restart` и `update`. (`fiber.ts` `_refresh`.)
- **R3. Запуск перехода.** Эпоха пересчитывается при каждом изменении зависимостей и ревизии. Если она
  изменилась, ошибки нет и переход не идёт: из пустой в непустую начинается загрузка, иначе выгрузка.
  (`fiber.ts` `_setEpoch`.)
- **R4. Инерция.** У fiber не больше одного перехода. Запросы во время перехода только меняют эпоху. По
  окончании загрузки: эпоха та же → `Active` (или путь ошибки R8); другая → выгрузка. По окончании выгрузки:
  эпоха пуста → покой (R1); непуста → загрузка. Поэтому серия запросов сходится к последнему.
  (fiber.spec «inertia lock 1–3»; update.spec «settles concurrent updates on the latest config»,
  «coalesces updates during unloading».)
- **R5. Фиксация и отложенный старт.** В начале загрузки ядро фиксирует набор зависимостей (committed view) и
  записывает fiber в потребители каждой чужой записи. `apply` стартует **в следующем ходе ядра** и только если
  эпоха не изменилась. (`fiber.ts` `_reload`; update.spec «skips superseded loads before executing the component»;
  reflect.spec «does not load a fiber disposed during service notification».)
- **R6. Шаги загрузки.** `ConfigResolver` → проверка схемы → разбор в `P::Config` → `Inject::resolve` из
  зафиксированного набора → `apply` в отдельной задаче. Ошибка любого шага есть ошибка активации.
  (fiber.spec «fail on first load».)
- **R7. `apply` не обрывается.** Замещённая активация доигрывает до возврата из `apply`; её контекст
  становится неактивным сразу (R17), поэтому следующий эффект вернёт `Inactive`, и `?` завершит функцию.
  Future `apply` ядро не роняет. (update.spec «stops advancing an outdated async generator and collects its
  final yield»; исследование, таблица 6.1, строка про генераторы.)
- **R8. Ошибка активации.** Ошибка пишется в журнал. Если эпоха не изменилась: ошибка сохраняется, эпоха
  становится пустой, эффекты снимаются через `Unloading`, итог `Failed`. Если эпоха изменилась, ошибка только
  записана и новую цель не портит. `Inactive` из замещённой активации пишется уровнем `Debug`. Паника в
  `apply` есть ошибка активации. (update.spec «cleans up a superseded failure without failing the latest
  update»; [D] cordis-core-lifecycle «cleans up a superseded activation failure and accepts the latest update».)
- **R9. `Failed` не лечится сам.** Пока ошибка хранится, смена зависимостей и `restart()` состояние не меняют.
  Выводит из `Failed` только `update()` (сбрасывает ошибку и перезагружает) или `dispose()`.
  (`_setEpoch`: ранний выход при ошибке; fiber.spec «failed fiber does not re-enter on dependency refresh»,
  «update recovers a failed fiber», «update waits for asynchronous recovery»; V25.)
- **R10. Выгрузка.** (а) Ждать, пока у каждой собственной записи сервиса не останется потребителей.
  (б) Снять все эффекты: запуск в порядке, обратном регистрации, исполнение одновременное, ошибка каждого в
  журнал. (в) Убрать себя из потребителей зафиксированных записей и разбудить их владельцев; отпустить набор.
  (г) Покой или новая загрузка по R4. (`fiber.ts` `_unload`.)
- **R11. `settled()`.** Ждёт, пока переходов нет. `Err(Failed)`, если fiber в `Failed`. Все, кто ждал
  последнюю загрузку, получают её ошибку. (`fiber.ts` `await`; update.spec «rejects all waiting updates when
  the latest load fails»; fiber.spec «update surfaces a failed recovery to its caller», «update surfaces a
  failed reload to its caller».)
- **R12. `update(raw)`.** На снятом fiber: `Inactive`. Если эпоха непуста, кандидат сначала разрешается и
  проверяется; при отказе `Err(Validation)`, экземпляр и конфигурация остаются прежними. Затем сырое значение
  заменяется, ошибка сбрасывается. Если эпоха непуста, исполняется цепочка обновления (R42), иначе `restart`.
  У `Pending` новое значение просто ждёт зависимостей. (`fiber.ts` `update`; fiber.spec «keep a running component
  on update»; [D] cordis-config-access «retains a pending raw update until its injection is available».)
- **R13. `restart()`.** Ревизия +1, пересчёт эпохи, ожидание покоя. Исполняется и во время `Loading`, и при
  неизменной конфигурации. (update.spec «honors restart / config ABA while loading even when config is
  unchanged».)
- **R14. `dispose()`.** Дочерний fiber зарегистрирован в списке эффектов родителя как `ctx.plugin()` ещё до
  рассылки `internal/plugin` (V6). Снятие: пометка «снят», повторная рассылка `internal/plugin`, эпоха
  пуста, ожидание переходов. Пути: `Pending → Disposed`, `Failed → Disposed`, `Active → Unloading → Disposed`.
  Повторный вызов ничего не делает. Отложенная конфигурация после снятия не загружается. (`fiber.ts`
  конструктор; fiber.spec «settles a pending fiber to DISPOSED on dispose», «settles a failed fiber to
  DISPOSED on dispose», «keeps the active dispose sequence stable»; update.spec «does not load a queued
  config after disposal»; component.spec «nested components».)
- **R15. Корень.** uid 0, всегда `Active`. `Kernel::shutdown()` снимает его эффекты по R10 и останавливает
  ядро; после этого любая операция возвращает `Closed`. Отличие от образца: там корень можно использовать
  дальше (component.spec «root dispose»), нам это не нужно.
- **R16. Уведомление о состоянии.** Каждая смена состояния рассылает `InternalStatus`. При входе в `Active` и
  выходе из него пересчитываются потребители всех записей fiber. (`fiber.ts` `_updateState`.)

### 3.2. Эффекты

- **R17. Активность контекста.** Эффект можно создать, только если fiber не снят, его состояние не
  `Unloading`, а контекст принадлежит текущей активации. Иначе `Inactive`, и `setup` не вызывается. Проверка и
  постановка в список неделимы. Контекст прошлой активации неактивен навсегда: это строже образца и закрывает
  утечку регистраций из задач старого экземпляра. `Loading` допустим. У fiber в покое контекста активации нет,
  поэтому часть V6 про эффекты, прикреплённые в `Pending`, неприменима. (component.spec «inactive context»; V6.)
- **R18. Однократность.** Обратное действие исполняется один раз. `Disposer::dispose` начинает снятие в момент
  вызова; если снятие уже идёт (выгрузка fiber), вызов присоединяется к нему. (V6; dispose.spec «dispose
  manually», «dispose by component».)
- **R19. Составной эффект.** В `try_effect` и `effect_async` обратные действия исполняются в порядке, обратном
  `push`, по одному. Ошибка `setup` запускает уже собранные и возвращается вызывающему. `Disposer`, переданный в
  `push`, уходит из списка fiber во владение составного эффекта. (`fiber.ts` `effect`: `collect`; dispose.spec
  «yield dispose», «return with error», «yield with error», «async return with error», «async yield with
  error»; service-dispose.spec «supports composed provider effects».)
- **R20. Незавершённый setup.** Эффект стоит в списке fiber до начала `setup`. Выгрузка, начатая во время
  `setup` (в том числе изнутри него), ждёт его окончания и затем исполняет всё собранное. (V6; dispose.spec
  «async return 1», «async return 2», «async yield 4 (await dispose)»; service-dispose.spec «retains
  dependencies through in-flight setup and rollback».)
- **R21. Прерывание между шагами.** Если эффект снимают во время асинхронного `setup`, ближайший `Undos::push`
  сохраняет переданное действие и возвращает `Inactive`; `setup` обязан выйти. Собранное исполняется.
  (dispose.spec «async yield 2 (aborted)», «async yield 3 (aborted)», «async yield (aborted, finally)».)
- **R22. Ошибки снятия.** Ошибка обратного действия пишется в журнал с меткой эффекта и не останавливает
  остальные; потребители и зависимости освобождаются всё равно. (fiber.spec «dispose error»;
  service-dispose.spec «releases dependencies after a cleanup error».)
- **R23. `Drop`.** Если `Undo` уничтожается неисполненным (остановка runtime без `shutdown`), в журнал идёт
  ошибка `effect "<метка>" of <fiber> dropped without disposal`. В `Drop` ничего не исполняется.
- **R24. `spawn`.** Задача есть эффект: снятие отменяет её токен и ждёт завершения. Ошибка задачи пишется в
  журнал. Задача обязана выходить по токену; работа, не привязанная к `ctx`, запрещена `PORTING.md` §6.

### 3.3. Сервисы

- **R25. Запись.** `provide` есть эффект предоставляющего fiber. Ключ записи: (`K::NAME`, область этого имени
  в предоставляющем контексте). Вторая запись с тем же ключом: `DuplicateService`. То же имя с другим `TypeId`
  ключа где угодно в ядре: `ServiceType`. (`reflect.ts` `provide`/`set`.)
- **R26. Видимость.** `get`, `has`, `view` видят запись, только когда её владелец `Active`. Записи корня видны
  всегда. Через неактивный контекст (R17) всё это возвращает «нет». (`reflect.ts` `_getImpl`, `strict = true`;
  reflect.spec «service inject leak».)
- **R27. Удовлетворение зависимости.** Имя разрешено, если в области потребителя есть видимая запись.
  (`fiber.ts` `_checkImpl`; service.spec «pending inject», «multiple injects».)
- **R28. Уведомление.** Появление и исчезновение записи, вход владельца в `Active` и выход из него пересчитывают
  эпохи всех fiber, объявивших это имя в этой области, и рассылают `InternalService`. (`reflect.ts` `notify`.)
- **R29. Зафиксированный набор.** Потребитель работает со значениями, зафиксированными при загрузке, до конца
  своей выгрузки. Замена поставщика приводит к выгрузке потребителя и новой загрузке с новым значением.
  ([D] cordis-core-lifecycle «separates own isolated provisions from the committed injection»; fiber.spec
  «update config while injected service reloads».)
- **R30. Поставщик ждёт потребителей.** Выгрузка владельца и снятие отдельной записи ждут, пока множество
  потребителей **этой** записи не опустеет. Запись перестаёт быть видимой сразу, чтобы новые потребители не
  привязались. (service.spec «dispose dependents before provider», «dispose dependents before provider
  restarts», «dispose dependents before isolated provider»; service-dispose.spec «waits for a disposing consumer
  that has already left the registry», «withdraws one binding without waiting for another binding from the same
  provider», «retains transitive dependencies while a consumer providing another service drains», «waits for
  current and withdrawn bindings during provider disposal», «keeps provider listeners alive through the final
  consumer event».)
- **R31. Замена записи.** Пока снятая запись ждёт потребителей, тот же fiber может предоставить новую под тем
  же ключом. (service-dispose.spec «withdraws only the old binding without waiting for the next load»,
  «replaces a binding from the same provider during consumer initialization».)
- **R32. Независимость.** Не связанные зависимостью поставщики и соседние эффекты снимаются одновременно.
  (service-dispose.spec «allows unrelated providers and sibling effects to clean up concurrently».)
- **R33. Дочерние fiber.** Дочерний fiber получает области, перехваты и метки создавшего контекста. Сервис
  родителя он использует, объявив его в своих `Deps`, либо получив `Service<K>` от родителя значением; набор
  родителя жив до конца выгрузки детей, потому что дети — его эффекты. (service-dispose.spec «keeps an
  inherited service accessible to a child in a composed effect», «retains an ancestor injection until its child
  finishes cleanup», «waits for a child that explicitly injects its parent service».)
- **R34. Типизированный доступ.** Значение обязательного сервиса можно получить только из `Deps`. `ctx.get`
  возвращает `Option` и гарантий R29–R30 не даёт: результат нельзя хранить дольше одного действия. Для
  длительной работы с необязательным сервисом служит `ctx.inject`. (reflect.spec «access check», «access
  error» становятся проверками компиляции.)
- **R35. Утечка ссылки.** После выгрузки владельца ядро смотрит `Arc::strong_count` снятого значения; если
  ссылки остались, пишет предупреждение `service "<имя>" is still referenced after its provider unloaded`.

### 3.4. Области и перехваты

- **R36. `isolate`.** Производный контекст, где имя смотрит в указанную область; остальные имена наследуются.
  Одно значение `Realm`, переданное из разных контекстов, даёт общую область. (isolate.spec «isolated context»,
  «shared label».)
- **R37. Граница.** Запись, сделанная через изолированный контекст, вне этой области не видна; одноимённые
  сервисы в разных областях отслеживаются независимо. (isolate.spec «does not expose own provisions across
  isolation boundaries»; service-dispose.spec «tracks same-name services in separate isolation realms»,
  «releases an injected binding after providing the same name in another realm».)
- **R38. Без запасного пути.** Поиск в области никогда не возвращает значение из другой области, в том числе
  из зафиксированного набора самого fiber. (isolate.spec «does not inherit the injected view across isolation
  boundaries», «keeps own provisions separate from the injected view»; [D] cordis-core-rc12 «isolates a context
  pinned to its parent realm label from the injected view»; V7.)
- **R39. `intercept`.** Параметры перехвата — объект JSON на имя сервиса. `intercepted` сливает слои от
  внешнего к внутреннему по ключам верхнего уровня и разбирает результат в `T`; при ошибке разбора
  `T::default()` и запись в журнал. Потребители в ядре: журнал (`name`, `level`). (intercept.spec «merge
  context intercepts from outer to inner».)

### 3.5. События

- **R40. Режимы.** (`events.ts`.)
  - `Emit`: слушатели по порядку, синхронно. Паника слушателя перехватывается, пишется в журнал с именем
    события, остальные вызываются. (events.spec «ctx.emit()»; V6, V7 про изоляцию наблюдателей.)
  - `Parallel`: все слушатели стартуют, все дожидаются; если были ошибки, `Listeners { event, errors }`.
    (events.spec «ctx.parallel()»; [D] cordis-core-rc12 «names the failing event in the parallel AggregateError».)
  - `Serial`: по одному; первый `Some` возвращается, остальные не вызываются; ошибка прерывает. («ctx.serial()».)
  - `Bail`: синхронно; первый `Some`. («ctx.bail()».)
  - `Waterfall`: цепочка. Слушатель получает `Next`; не вызвал — цепочка оборвана его результатом; после
    последнего слушателя исполняется `tail`. («ctx.waterfall()», «ctx.waterfall() supports nested async calls».)
- **R41. Порядок и снимок.** Порядок регистрации; `prepend` ставит первым. Рассылка работает по снимку списка:
  слушатель, снятый во время рассылки, пропускается; добавленный ждёт следующей. (events.spec «ctx.on()»,
  «%s skips listeners removed during dispatch», «%s defers added listeners until the next dispatch», «skips a
  listener disposed by an earlier component during dispatch», «waterfall continues past a removed listener»,
  «waterfall defers added listeners until the next dispatch».)
- **R42. Цепочка обновления.** Обработчики `on_update` принадлежат fiber и видят только его обновления.
  Исполняются по порядку (R41); обработчик, не вызвавший `next`, принимает обновление на месте: экземпляр
  сохраняется, `Cfg` получает новый снимок. Последнее звено — `restart`. После перезагрузки старые обработчики
  сняты вместе с эффектами. Обновления одного fiber исполняются по очереди. (events.spec «internal/update: does
  not accumulate across reloads», «can be disposed explicitly», «runs prepended listeners first»,
  «internal/update continues past a removed fiber listener»; update.spec «does not restart when a listener
  accepts an update during loading»; fiber.spec «keep the source config when a listener handles the update».)
- **R43. Слушатель есть эффект** регистрирующего fiber. (component.spec «compare snapshot»; events.spec
  «removes empty event buckets after disposal».)
- **R44. Фильтр.** Рассылка из `ctx.with_filter(f)` доходит только до слушателей, чей контекст регистрации
  удовлетворяет `f`; слушатели `global` получают всё. Так переносится `thisArg[Context.filter]`: область
  агента в `[D] packages/core/scope` (метка `with_tag` + фильтр) и рассылка «от имени сервиса» в пределах его
  области (`realm_of`). (`events.ts` строки 168–170; isolate.spec «isolated event».)
- **R45. `internal/dispatch`.** Перед каждой рассылкой события, имя которого не начинается с `internal/`,
  вызываются наблюдатели `on_dispatch` с режимом, именем и самим событием. (`events.ts` строка 165.)
- **R46. События ядра** `InternalPlugin`, `InternalStatus`, `InternalService`, `VolatileUpdate` рассылает
  задача ядра. Их слушатели не блокируют и не ждут; операции ядра из них разрешены (R59). `VolatileUpdate`
  получают только слушатели, зарегистрированные контекстами целевого fiber. (V6; V22.)

### 3.6. Конфигурация

- **R47. Отказ.** Текст `ValidationError`: `invalid config:` и по строке `  - <сообщение> (at <путь>)` на
  замечание; без пути скобок нет. Путь: сегменты через точку, индексы числами. (fiber.spec «keep the issues and
  their paths»; [D] cordis-core-rc12 «formats config validation paths with keyed segments».)
- **R48. Проверка.** `config::validate` обходит схему, порождённую `schemars`, и значение. Закрытый набор слов:
  `type`, `enum`, `const`, `properties`, `required`, `additionalProperties`, `items`, `prefixItems`, `minItems`,
  `maxItems`, `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`, `minLength`,
  `maxLength`, `pattern`, `anyOf`, `oneOf`, `allOf`, `$ref` на `#/$defs/..`, булевы схемы. Слова-аннотации
  (`title`, `description`, `default`, `format`, `examples`, `deprecated`, `x-*`, `$schema`, `$defs`)
  пропускаются. Любое другое слово в схеме плагина есть ошибка `PluginDef::of`, то есть первый же тест её
  поймает. Собираются все замечания, а не первое.
- **R49. Тексты замечаний** повторяют schemastery (`[D] vendor/schemastery/src/index.ts`): `expected <тип> but
  got <значение>`, `expected number >= <min> but got <v>`, `expected number <= <max> but got <v>`, `expected
  number multiple of <step> but got <v>`, `expect string to match regexp <re>`, `missing required value`,
  для `enum`/`const`/`anyOf`/`oneOf`: `expected <описание> but got <JSON значения>`. Значение печатается так,
  как его печатает JavaScript в шаблонной строке; расхождения записываются в `PORT_NOTES.md` ядра.
- **R50. Допуски.** Неизвестные ключи объекта принимаются и не доходят до типа (schemastery `object` вне
  строгого режима сохраняет их). `null` и отсутствие значения на месте корневого объекта равны `{}`.
  Умолчания задаёт serde (`#[serde(default)]`), в схему они попадают через `schemars`.
- **R51. Три значения конфигурации.** `raw_config` — то, что передано, без изменений, даже когда обновление
  принял обработчик. `effective_config` — разрешённое и проверенное значение работающего экземпляра.
  `resolve_config(кандидат)` — чистый расчёт без побочных действий; неудачный или замещённый расчёт ничего не
  меняет. (V15; fiber.spec «pass the resolved config to listeners», «keep the source config when a listener
  handles the update»; [D] cordis-config-access, все четыре случая.)
- **R52. Проверка кандидата плагином.** Обработчики `on_candidate` вызываются после проверки схемы при
  `update`, `commit_volatile` и `resolve_config` своего fiber. `Err` становится замечанием без пути. Так
  переносятся слушатели `internal/config` в `mcp-manager` и `llm-pi-ai`. Обработчики синхронны и не блокируют:
  при `update` их вызывает задача ядра. Тип `C` в `on_candidate` и `on_update` обязан совпадать с `P::Config`,
  иначе регистрация возвращает `ConfigType`.
- **R53. Volatile.** `commit_volatile(raw)`: fiber должен быть `Active`. Кандидат разрешается и проверяется;
  если значения вне путей `x-volatile` отличаются от `effective_config`, возвращается `Ok(false)` без
  изменений. Иначе `raw_config`, `effective_config` и снимок `Cfg` заменяются целиком, рассылается
  `VolatileUpdate { paths }`, цепочка R42 не вызывается, результат `Ok(true)`. Неверный кандидат: `Err`,
  сырое значение остаётся для следующей активации. Плагин читает volatile-поля через `cfg.get()` в момент
  использования. (V22; [D] loader-volatile-update «commits all references before one local event without
  remounting», «bypasses custom update hooks for volatile changes and retains them for mixed changes», «logs an
  invalid candidate and keeps its references while other entries commit».)
- **R54. Сравнение.** `equal_except_volatile(schema, a, b)`: строгое сравнение сырых значений с пропуском путей
  `x-volatile`; отсутствующий объект равен объекту из умолчаний; возвратные ссылки схемы сравниваются сырыми.
  Обработчики и проверка не вызываются. ([D] loader-config-diff, все одиннадцать случаев.)

### 3.7. Журнал и время

- **R55. Имя журнала.** `ctx.logger()` берёт имя из перехвата `logger.name`, иначе имя fiber в kebab-case;
  `named` задаёт его явно. Запись уходит приёмнику, если `levels.by_name[имя] ?? levels.default ??
  уровень из перехвата ?? Info` не меньше уровня записи. (`logger.ts` `_method`, `[symbols.invoke]`; logger.spec
  «uses fiber name when called from outside any service», «honours explicit name argument», «honours intercept
  name».)
- **R56. Буфер.** Первый приёмник ядра — кольцевой буфер на 1000 записей в порядке поступления. (logger.spec
  «keeps the bounded buffer in place and chronological», «disposes the exporter that registered the disposer».)
- **R57. Составная ошибка.** `Listeners` с непустым списком пишется в журнал по одной записи на вложенную
  ошибку, с пустым — одной записью. (logger.spec «flattens an aggregate error into its members», «logs an error
  whose errors array is empty».)
- **R58. Время.** `sleep` завершается `Ok` по истечении срока либо `Err(Inactive)`, когда активация перестала
  быть текущей. `interval` выдаёт по элементу на тик и заканчивается в тот же момент. Оба построены на
  `tokio::time`, поэтому `tokio::time::pause` в тестах работает. ([D] `vendor/timer`; cordis-timer.spec «settles
  all pending interval reads on dispose».)

### 3.8. Локальные правки dsh

| Правка | Где учтена |
|---|---|
| V6: повторный вход при снятии | R14, R17–R22, R46, R61 |
| V7 (документация договорённостей): `isolate` без запасного пути; сбой одного наблюдателя не мешает другим | R38, R40 |
| V11: один алгоритм патчей для запуска и для выдачи состава; диалект YAML | L1, L2, L9, L10, L18 |
| V15: сырая, действующая и расчётная конфигурация; носители без разрешения выражений | R51, L6 |
| V22: volatile | R53, R54, L13 |
| V25: отключённая строка дожидается снятия; `Failed` требует явного `update` | R9, L13. Обновление зависимостей перед совместной сменой контекста и конфигурации неприменимо: смена контекста пересоздаёт строку |
| V27: id с двоеточием | L4 |
| V28: строки Loader и Include под rc.12 | вошло в основу: правила ядра взяты из rc.13 |
| V14, V26: сохранение состава в файл, перемещение строки между деревьями | не переносится (O4, L16) |
| V1–V5, V9, V10, V16, V17, V23, V24 | сборка npm, HMR, имена пакетов, локали: к ядру на Rust не относятся |

Номеров V8, V12, V13, V18–V21 в журнале нет.

## 4. Автомат fiber и модель одновременности

### 4.1. Переходы

| Из | Событие | В | Что делает ядро |
|---|---|---|---|
| — | `plugin`, `plugin_dyn`, `inject` | `Pending` | эффект `ctx.plugin()` у родителя; `InternalPlugin`; расчёт эпохи |
| `Pending` | эпоха стала непустой | `Loading` | фиксация набора, запись в потребители; команда «старт» в свою очередь |
| `Loading` | «старт», эпоха прежняя | `Loading` | R6: разбор конфигурации, задача `apply` |
| `Loading` | «старт», эпоха другая | `Unloading` | `apply` не вызывается |
| `Loading` | `apply` вернул `Ok`, эпоха прежняя | `Active` | пересчёт потребителей своих записей |
| `Loading` | `apply` вернул `Ok`, эпоха другая | `Unloading` | — |
| `Loading` | ошибка, эпоха прежняя | `Unloading` | сохранить ошибку, эпоха пуста |
| `Loading` | ошибка, эпоха другая | `Unloading` | ошибка только в журнал |
| `Active` | эпоха изменилась (зависимость, `restart`, `update` → `restart`, `dispose`) | `Unloading` | закрыть контекст активации; отменить `token`; пересчёт потребителей |
| `Unloading` | эффекты сняты, эпоха пуста, ошибки нет | `Pending` | — |
| `Unloading` | эффекты сняты, эпоха пуста, ошибка есть | `Failed` | — |
| `Unloading` | эффекты сняты, эпоха непуста | `Loading` | как строка 2 |
| `Unloading` | эффекты сняты, fiber снят | `Disposed` | удалить из таблицы |
| `Pending`, `Failed` | `dispose` | `Disposed` | удалить из таблицы |
| `Failed` | `update` | `Pending` или `Loading` | сбросить ошибку, ревизия +1 |

`Loading` при изменившейся эпохе закрывает контекст активации сразу, не дожидаясь возврата из `apply` (R7).

### 4.2. Что где исполняется

**Задача ядра** (одна, обычная `tokio::spawn`; состояние `Send`): чтение очереди команд, таблица fiber и их
состояния, эпохи, таблица сервисов с потребителями, разбор и проверка конфигурации, рассылка событий ядра.
Она ничего не ждёт, кроме своей очереди, и не вызывает асинхронный код плагинов.

**Задачи fiber**: на каждую загрузку одна задача с `apply`; на каждую выгрузку одна задача, которая ждёт
незавершённые `setup`, исполняет обратные действия и сообщает ядру итог. Паника задачи доходит до ядра как
`JoinError` и становится ошибкой.

**Где угодно** (потоки плагинов): `effect`, `try_effect`, `on*`, `emit`, `bail`, `get`, `has`, `view`,
`intercepted`, `tag`, `logger` — синхронные операции над общими структурами:

| Структура | Защита | Кто пишет | Кто читает |
|---|---|---|---|
| опубликованная таблица сервисов и состояний | `std::sync::RwLock` | только задача ядра | `get`, `has`, `view`, `Fiber::state`, `raw_config` |
| список эффектов активации с флагом «закрыт» | `std::sync::Mutex` | создатель эффекта; ядро закрывает | задача выгрузки |
| шина событий: имя → список слушателей | `std::sync::RwLock` | регистрация и снятие слушателя | рассылка берёт снимок под блокировкой, вызывает вне её |
| журнал: буфер и приёмники | `std::sync::Mutex` | любой | любой |

Блокировки не удерживаются через `.await` и во время вызова чужого кода.

### 4.3. Как исключён повторный вход

- **R59. Команды, а не вызовы.** Любая операция, меняющая жизненный цикл (`plugin`, `provide`, `dispose`,
  `update`, `restart`, `commit_volatile`, снятие записи сервиса), кладёт команду в очередь ядра в момент вызова
  и сразу возвращает управление. Команды исполняются по одной, в порядке поступления. Слушатель события ядра,
  вызвавший такую операцию посреди перехода, получит её исполнение после окончания текущего хода. Отсюда же
  следует порядок в reflect.spec «does not load a fiber disposed during service notification»: `dispose`
  встаёт в очередь раньше «старта» (R5). Для этого команда «старт» ставится в очередь только после рассылки
  `InternalStatus` о входе в `Loading`.
- **R60. Ожидание себя.** `settled`, `restart`, `update`, `dispose` собственного fiber или предка, ожидаемые
  из задачи `apply` этого fiber, не могут завершиться. Ядро узнаёт такую задачу по `tokio::task_local!` с uid
  и возвращает `SelfWait` вместо зависания. Вызвать без ожидания можно: команда уже отправлена.
- **R61. События ядра** рассылаются без удержания блокировки опубликованной таблицы; паника слушателя
  перехватывается (R40).
- **R62. Отказ ядра.** Паника задачи ядра есть ошибка реализации: все ожидающие получают `Closed`.

## 5. Загрузчик

Источники: `[D] vendor/loader`, `vendor/include`, `vendor/group`, `packages/boot/app-boot`, `cmdline`,
`config-editor`; статья §5.2.1.

### 5.1. Формат

- **L1. Диалект YAML.** Чтение идёт по событиям `saphyr-parser`; значение сразу строится как
  `serde_json::Value` с порядком ключей. Скаляр без кавычек разбирается по правилам js-yaml 4 `JSON_SCHEMA`:
  `null`, `Null`, `NULL`, `~`, пустой — `null`; `true`/`false` в трёх регистрах — логические; целые десятичные,
  `0x`, `0o`, `0b`; дробные с точкой или экспонентой; всё остальное — строка. Дат нет, `<<` — обычный ключ.
  Ключ отображения только строка; повтор ключа — ошибка. Якоря и ссылки читаются копированием. Скаляр с тегом
  `!!js` становится узлом `{"__jsExpr": "<текст>"}`; объект с единственным ключом `__jsExpr` тем самым
  зарезервирован. Ошибка несёт файл, строку и столбец. Точные границы числовых форм сверить с
  `js-yaml/lib/type/{int,float}.js` образца. (`[D] vendor/include/src/index.ts` `parseEntryYaml`; V11;
  cordis-entry-yaml «reads and writes !!js scalars as expression nodes», «reads binary integers as numbers and
  dates as strings», «locates parse errors».)
- **L2. Запись YAML.** Собственный вывод: блочный стиль, отступ два пробела, порядок ключей сохранён, строки в
  двойных кавычках кроме простых идентификаторов, узел выражения печатается как `!!js "<текст>"`, якорей нет.
  (cordis-entry-yaml «writes repeated references in full».)
- **L3. Файлы.** Верхний уровень файла — массив: строк либо патчей. Слой профиля и слой домашнего каталога
  могут отсутствовать. Файл, названный явно (пакет, `--patch`), обязан существовать. Нечитаемый, неразборный
  файл или не массив — отказ запуска, а не пропуск. (`[D] app-boot/src/index.ts` `loadOptionalPatches`,
  `loadOverlayPatches`.)
- **L4. Ключи строки.**

  | Ключ | Значение | Смысл |
  |---|---|---|
  | `id` | строка | Ключ строки при сравнении составов. Пустой заменяется восемью шестнадцатеричными знаками, уникальными в дереве. Двоеточие в id допустимо (V27). |
  | `name` | строка | Имя плагина: имя HoH, псевдоним dsh (`@deepseek-ai/dsh-tools`, с подпутём `…/tools`), `cordis:group`, `cordis:include`. |
  | `config` | любое | Сырая конфигурация плагина; у группы — список строк. Может содержать выражения. |
  | `disabled` | логическое, `null` или выражение | Истина: fiber не создаётся. |
  | `group` | логическое | Строка — группа: `config` есть список строк, патчи видят её детей. |
  | `inject` | список имён | Дополнительные обязательные сервисы строки (`inject: [headlessStartup]`). |
  | `isolate` | `{ сервис: true \| "метка" }` | `true`: область, принадлежащая этой строке. Строка: область, общая для всех строк загрузчика с этой меткой. |
  | `intercept` | `{ сервис: объект }` | Слой перехвата для поддерева строки. В поставке dsh не встречается. |

  Неизвестный ключ строки — ошибка формы. (`[D] vendor/loader/src/config/entry.ts` `EntryOptions`,
  `config/isolate.ts`, `config/tree.ts` `ensureId`.)
- **L5. Имя плагина.** Поиск: точное имя в таблице, затем псевдоним. Относительное или абсолютное имя во
  вставленной строке привязывается к каталогу файла патча (`file://…`) и ищется в этом виде. Имя не найдено:
  статус `Unresolved`, ошибка в журнал, остальные строки продолжают загружаться.
  (`[D] app-boot/src/index.ts` `anchorInsertedPluginNames`.)

### 5.2. Строка, группа, включение

- **L6. Создание строки.** (1) вычислить `disabled`; (2) найти плагин; (3) построить контекст: контекст группы
  плюс `isolate` и `intercept` строки; (4) `plugin_dyn` с `entry_id`, `inject` и `resolver`. Носители (L7)
  `resolver` не получают: выражения во вложенных строках ждут своих строк и своих зависимостей (V15).
- **L7. Группа и носитель.** `cordis:group` монтирует `Group` под своим контекстом, поэтому `isolate` группы
  даёт одну область поставщику и потребителям внутри неё. Обновление списка группа принимает на месте
  (`on_update` без `next`) и передаёт в `Group::update`. Носителем считается и плагин, помеченный
  `PluginRegistry::carrier` (в dsh: `agent-preset` со статическим `EntryGroup.key`).
- **L8. Включение.** `cordis:include` с конфигурацией `{ path, patches }` читает файл строк, применяет патчи
  (L10) и монтирует результат как группу. Изменение `patches` или `path` пересчитывает строки и идёт в
  `Group::update`. Обратная запись изменений в файл не переносится (вопрос O4).
- **L9. Слои.** Порядок применения: (1) пакеты из `dsh.profile.bundles` файла `<профиль>/package.json`, каждый
  со своими файлами патчей в объявленном порядке; (2) `<профиль>/cordis.patch.yml`; (3) `<дом>/cordis.patch.yml`;
  (4) `--patch <путь>` в порядке аргументов; (5) патчи запускающего кода (в dsh: отключение телеметрии по
  переменной окружения, если строка есть в составе). Пакет, который не найден, не подходит платформе или не
  читается, пропускается с записью в `Profile::skipped`. Все слои склеиваются в один список и применяются
  **одним** вызовом `apply_patches` к строкам базового файла. (`[D] app-boot/src/profile.ts`
  `loadProfileDirectory`, `composeEntries`; `profile-context.ts` `readProfilePatches`; V11.)
- **L10. `apply_patches`.** Данные копируются. Индекс «id → строка» строится рекурсивно через строки с
  `group: true` и списком в `config`. Для каждого патча по порядку:
  - есть `insert`: с `id` — цель должна существовать (`patch insert: entry <id> not found`) и быть группой
    (`patch insert: entry <id> is not a group`), строки добавляются в конец её списка; без `id` — в конец
    корня. Вставленные строки попадают в индекс, и поздние патчи их видят;
  - нет `insert`: `id` обязателен (`patch: id is required for non-insert patches`); цель должна существовать
    (`patch: entry <id> not found`); `name`, не равный имени цели, отменяет патч (`patch: name mismatch for <id>
    (expected <имя цели>, got <имя патча>), skipping`); иначе каждый ключ из `set` заменяет ключ строки целиком,
    `null` означает «ключа нет».

  Каждое предупреждение пропускает один патч и не останавливает остальные. (`[D] vendor/include/src/patch.ts`
  `applyPatches`; cordis-entry-commit «detaches file entries even with no patches», «clones inserted groups and
  lets later patches address their descendants».)

### 5.3. Перекомпоновка

- **L11. `recompose(layers)`.** Новый состав считается по L9–L10 и передаётся корневому включению как новое
  значение `patches`; дальше работает L12. Вызывающий (редактор конфигурации) сначала пишет файл патча, затем
  зовёт `recompose`; при отказе возвращает прежний файл и зовёт снова. (`[D] app-boot/src/index.ts`
  `reconcileProfilePatches`; `config-editor/src/index.ts`.)
- **L12. `Group::update(rows)`.** Сравнение по `id`. Все id старого и нового списков обрабатываются
  одновременно; ошибка одной строки пишется в журнал и не мешает остальным. Исчезнувший id: fiber снимается.
  Остальные: L13. (`[D] vendor/loader/src/config/group.ts` `update`.)
- **L13. Действие для строки.** Выбирается первое подходящее.

  | Что изменилось | Действие |
  |---|---|
  | `disabled` стало истиной | снять fiber и дождаться (V25) |
  | fiber нет или он снят | создать (L6) |
  | `name`, `isolate`, `intercept`, `inject` | пересоздать: снять, дождаться, создать |
  | только `config`, fiber `Active`, `equal_except_volatile` | `commit_volatile`; при `Ok(false)` — следующая строка таблицы |
  | `config` | `fiber.update(config)`: плагин примет на месте (R42) или перезапустится |
  | fiber в `Failed` | `fiber.update(config)` (R9) |
  | ничего | ничего; группа всё равно получает свой список и сравнивает детей |

  Отличие от образца: там смена `isolate` и `intercept` меняет области существующего контекста на месте
  (`_patchContext`, статья, алгоритм 7). У нас контекст неизменяем, поэтому строка пересоздаётся (вопрос O3).
  (`[D] vendor/loader/src/config/entry.ts` `update`, `_commitVolatile`; V22; V25; loader-volatile-update.)
- **L14. Условия не реактивны.** `disabled` вычисляется при создании строки и при каждой перекомпоновке.
  Появление сервиса само условие не пересчитывает. Сервисы, от которых зависят условия (`profileContext`),
  запускающий код предоставляет до `boot`.
- **L15. Выражения в `config`** вычисляет `resolver` при каждой активации и для каждого кандидата обновления.
  `undefined` на месте значения ключа удаляет ключ, в массиве становится `null`.
- **L16. Правки дерева при работе.** `Loader::create` и `remove` действуют в памяти и в файлы не пишутся.
  Перемещения строк между группами нет: в dsh его никто не вызывает.
- **L17. `settled()`** ждёт, пока ни у одной строки нет идущего перехода. `Pending` считается покоем.

### 5.4. Выдача состава и проверка запуска

- **L18. `render_dump`.** Документ YAML со списком строк состава. Перед каждой строкой верхнего уровня
  комментарий `# == <id> · <слой-источник>` и, если строку меняли, ` · patched by <слои через запятую>`.
  Выражения печатаются как в исходнике. Предупреждения L10 идут в stderr с меткой слоя. `hoh --dump-config`
  печатает именно это и ничего не монтирует. (`[D] app-boot/src/index.ts` `renderConfigDump`.)
- **L19. `audit(required)`.** `missing`: обязательные id, которых нет в составе. `unresolved`: строки с
  ненайденным плагином. `inactive`: строки, чей fiber не `Active`, с недостающими сервисами и ошибкой. Список
  обязательных id задаёт приложение. (`[D] app-boot/src/index.ts` `auditStartupEntries`,
  `startup-diagnostics.ts`.)

### 5.5. Выражения: закрытый набор

JavaScript не исполняется. Текст после `!!js` разбирается небольшим распознавателем в терм из закрытого набора.
Текст вне грамматики — ошибка загрузки слоя с файлом и строкой. Все выражения слоя разбираются при его
чтении, вычисляются позже (L14, L15). Запись `!!js` сохранена, чтобы файлы dsh читались без правок; другой
записи нет.

```
expr    := or ( '?' expr ':' expr )?
or      := and ( ( '||' | '??' ) and )*
and     := eq ( '&&' eq )*
eq      := add ( ( '===' | '!==' ) add )?
add     := unary ( '+' unary )*
unary   := '!' unary | primary
primary := literal | '(' expr ')' | '[' ( expr ( ',' expr )* )? ']' ( '.includes(' expr ')' )? | head
literal := строка в одинарных или двойных кавычках | число | true | false | null | undefined
head    := process.env.ИМЯ | process.cwd() | process.execPath | process.platform
         | dshHomePath( строка ( ',' строка )* )
         | ctx.get( строка ) ( '?.' поле ( '.' поле )* )?
         | ctx. сервис ( '.' поле )+
         | Number( expr ) | JSON.parse( expr )
         | P.join( expr ( ',' expr )* ) | P.resolve( expr ) | P.dirname( REQ )
         | decodeURIComponent( new URL( строка , expr ).pathname )
P       := process.getBuiltinModule('node:path')
REQ     := process.getBuiltinModule('node:module').createRequire(baseUrl).resolve( строка )
```

| Терм | Значение |
|---|---|
| `process.env.X` | `EvalEnv::var`; нет переменной — `undefined`. Наложение файлов `.env`, которое в dsh делает `app-boot`, выполняет запускающий код до `boot` |
| `process.cwd()`, `process.execPath`, `process.platform` | `EvalEnv::cwd`, `exe`, `platform` |
| `dshHomePath(a, b, …)` | `EvalEnv::home` с добавленными сегментами |
| `ctx.get('s')` | `ctx.view("s")`; сервис есть, но данных не даёт — пустой объект; сервиса нет — `undefined` |
| `ctx.get('s')?.a.b`, `ctx.s.a.b` | поле данных сервиса. Форма `ctx.s…` при отсутствии сервиса — ошибка: строка обязана объявить `inject: [s]` |
| `[…]`, `[…].includes(e)` | массив; принадлежность по `===` |
| `!`, `&&`, `\|\|`, `??`, `? :`, `===`, `!==` | как в JavaScript: `&&` и `\|\|` возвращают операнд; ложны `false`, `null`, `undefined`, `0`, `""`; `===` только для строк, чисел, логических, `null`, `undefined` |
| `a + b` | склейка строк. `undefined` в склейке — ошибка (в JavaScript вышла бы строка `"undefined…"`) |
| `Number(e)` | число из строки или числа; не число — ошибка |
| `JSON.parse(e)` | разбор JSON из строки |
| `P.join(…)`, `P.resolve(e)` | склейка сегментов с нормализацией; абсолютный путь от `cwd` |
| `P.dirname(REQ)` со строкой `'<пакет>/package.json'` | каталог ресурсов пакета: `PluginRegistry::resource_dir` по имени или псевдониму |
| `decodeURIComponent(new URL(rel, 'file://' + e).pathname)` | путь `rel`, взятый от каталога файла `e` |

Данные сервисам, которые читают выражения, даёт `ServiceKey::view`: в поставке это `profileContext` (`name`),
`headlessStartup` (`task`, `sessionId`, `json`, `additionalWritableRoots`), `webStartup` (`trustedHosts`,
`tls`, `publicUrl`, `port`, `openBrowser`, `host`).

Выражения, которые набор заменяет. Просмотрены `packages/bundle/**/*.yml` (включая `web-app/presets`) и
`snapshots/**`: 379 вхождений, 76 различных выражений. `E.X` ниже сокращает `process.env.X`.

```
# каталог дома (13)
dshHomePath('sessions')                         dshHomePath('office-kit', 'outputs')
dshHomePath('storages')                         dshHomePath('dsh-runtimes', 'dsh-primary-runtime')
dshHomePath('team-requests.jsonl')              dshHomePath('plan-requests.jsonl')
dshHomePath('permission-requests.jsonl')        dshHomePath('goal-requests.jsonl')
dshHomePath('compact-requests.jsonl')           dshHomePath('command-identity-requests.jsonl')
dshHomePath('command-attachment-requests.jsonl')
dshHomePath('children/first')                   dshHomePath('children/second')

# переменная окружения как есть (13)
E.DSH_SNAPSHOT_SPILL_ROOT   E.DSH_SNAPSHOT_SPILL_LOCATOR_ROOT   E.DSH_MCP_DEMO_URL   E.DSH_TEST_CHILD_PATCH
E.DSH_SNAPSHOT_PROVIDER_CWD E.DSH_CLIENT_VERSION   E.DSH_TEST_CHILD_HOME   E.DSH_SNAPSHOT_PROVIDER
E.DSH_SNAPSHOT_MODEL        E.DSH_PRODUCT_ANALYTICS_OTLP_URL    E.DSH_PRIMARY_RUNTIME
[E.DSH_MCP_RESOURCES_FIXTURE]                   !E.DSH_PRIMARY_RUNTIME

# переменная с умолчанием (9)
E.DSH_SNAPSHOT_SPILL_ROOT ?? './.spill'         E.DSH_PERMISSION_MODE ?? 'workspace-write'
E.DSH_SNAPSHOT_SESSIONS_ROOT ?? './.sessions'   E.DSH_SNAPSHOT_SESSIONS_ROOT ?? dshHomePath('sessions')
E.DSH_SYSTEM_PROMPT ?? 'You are a helpful software engineer assistant.'
E.DSH_TELEMETRY_OTLP_URL ?? 'https://dsh-otel-collector.deepseeksvc.com/v1/logs'
E.DSH_TELEMETRY_MODE || 'FEEDBACK_ONLY'         E.DSH_MODEL ?? 'deepseek-v4-flash'
E.DSH_PERMISSION_MODE ?? (E.DSH_SNAPSHOT === undefined ? 'workspace-write' : 'danger-full-access')

# условие по переменной (10)
E.DSH_SNAPSHOT === undefined ? 'zstd' : 'none'  E.DSH_SNAPSHOT !== 'record'
E.DSH_SNAPSHOT_FEEDBACK !== '1'                 E.DSH_SNAPSHOT && 200 || 12500
E.DSH_PRIMARY_RUNTIME ? 'workspace-dependencies' : 'user'
(E.DSH_PERMISSION_MODE ?? 'workspace-write') === 'danger-full-access' ? 'never' : 'ask'
(E.DSH_PERMISSION_MODE ?? (E.DSH_SNAPSHOT === undefined ? 'workspace-write' : 'danger-full-access')) === 'danger-full-access' ? 'never' : 'ask'
E.DSH_MAX_TOKENS_AS_SUCCESS === undefined ? true : JSON.parse(E.DSH_MAX_TOKENS_AS_SUCCESS)
Number(E.DSH_CONTEXT_WINDOW ?? 1000000)         JSON.parse(E.DSH_TEST_CHILD_PATCHES ?? '[]')

# процесс (10)
process.cwd()                                   process.execPath
process.cwd() + '/office-skills'                process.cwd() + '/office-ptc-node'
process.cwd() + '/runtime-source'               process.cwd() + '/runtime-installed'
process.cwd() + '/native-helper.mjs'            E.DSH_SNAPSHOT_SPILL_ROOT + '/dsh-escalated.txt'
process.platform === 'win32'                    process.platform !== 'win32'

# пути (6)
P.join(process.cwd(), 'broken-skills')          P.join(process.cwd(), 'approved.txt')
P.resolve(E.DSH_PRIMARY_RUNTIME ?? '')
P.join(P.dirname(REQ('@deepseek-ai/dsh-agent-preset/package.json')), 'skills')
decodeURIComponent(new URL('./runner.mjs', 'file://' + E.DSH_SNAPSHOT_FILE).pathname)
decodeURIComponent(new URL('../../../packages/subagent/subagent-acp/tests/mock-acp-server.ts', 'file://' + E.DSH_SNAPSHOT_FILE).pathname)

# сервисы (15)
!ctx.get('profileContext')                      ctx.get('profileContext')?.name !== 'desktop'
ctx.get('profileContext')?.name !== 'desktop' || !E.DSH_PRIMARY_RUNTIME
ctx.get('profileContext')?.name === 'desktop' && E.DSH_PRIMARY_RUNTIME ? 'workspace-dependencies' : 'user'
ctx.get('profileContext')?.name === 'desktop' && ['darwin', 'win32'].includes(process.platform) ? process.platform : null
ctx.headlessStartup.task        ctx.headlessStartup.sessionId   ctx.headlessStartup.json
ctx.headlessStartup.additionalWritableRoots
ctx.webStartup.trustedHosts     ctx.webStartup.tls              ctx.webStartup.publicUrl
ctx.webStartup.openBrowser      ctx.webStartup.port ?? 3080     ctx.webStartup.host ?? '127.0.0.1'
```

Вне просмотренного остались `!!js` в тестовых профилях `apps/cli/tests`, фикстурах пакетов и примерах (в
репозитории всего 501 строка с `!!js`). Если там найдётся новая форма, загрузка откажет с её текстом, а набор
дополняется строкой в этом разделе.

Строки сценариев, которые называют модули JavaScript (`name: './feedback-producer.mjs'`, `command: !!js
process.execPath` с `runner.mjs`), читаются, но плагина для них нет: по L5 они получают `Unresolved`. Что с
ними делать, решает `hoh-conformance` (`conformance/EXPECTED.tsv`).

## 6. Три примера

Перенос `[D] docs/cordis-tutorial/03-services.md` и `07-into-the-harness.md`. Вместо `console.log` журнал:
`PORTING.md` §8 запрещает `println!`.

### 6.1. Поставщик сервиса

```rust
// crates/hoh-greeter/src/lib.rs
use std::sync::Arc;
use hoh_kernel::{plugin, service, Context, Error};

pub struct GreeterService;

impl GreeterService {
    pub fn greet(&self, who: &str) -> String {
        format!("Hello, {who}!")
    }
}

// `declare module` + `super(ctx, 'greeter')` из образца: ключ, имя и тип значения в одном месте.
service!(pub Greeter: GreeterService = "greeter");

#[plugin(GreeterPlugin, name = "greeter")]
pub async fn apply(ctx: Context) -> Result<(), Error> {
    // Предоставление есть эффект: выгрузка плагина убирает сервис.
    ctx.provide::<Greeter>(Arc::new(GreeterService)).await?;
    Ok(())
}
```

Потребители увидят `greeter`, когда `apply` вернёт `Ok` и fiber станет `Active` (R26).

### 6.2. Потребитель с `inject`

```rust
// crates/hoh-consumer/src/lib.rs
use hoh_greeter::Greeter;
use hoh_kernel::{plugin, Context, Error, Inject, Service};

#[derive(Inject)]
pub struct Deps {
    greeter: Service<Greeter>,
}

#[plugin(Consumer, name = "consumer")]
pub async fn apply(ctx: Context, deps: Deps) -> Result<(), Error> {
    ctx.logger().info(deps.greeter.greet("world"));
    Ok(())
}

// Необязательная зависимость: без `Deps`, явный запрос в месте использования.
#[plugin(MaybeConsumer, name = "maybe-consumer")]
pub async fn apply_optional(ctx: Context) -> Result<(), Error> {
    let text = match ctx.get::<Greeter>() {
        Some(greeter) => greeter.greet("maybe"),
        None => "no greeter available".to_owned(),
    };
    ctx.logger().info(text);
    Ok(())
}
```

Состав (порядок строк не важен, без `hoh-greeter` потребитель остаётся `Pending`):

```rust
let mut registry = PluginRegistry::default();
registry.add::<GreeterPlugin>("hoh-greeter").add::<Consumer>("hoh-consumer");
```

```yaml
- name: hoh-consumer
- name: hoh-greeter
```

### 6.3. Инструмент и слушатель события

Типы `Tools`, `ToolsResult`, `define_tool`, `ExecuteRequest`, `ToolCall` принадлежат будущему `hoh-tools`; их
вид здесь условен. Пример показывает идиомы ядра.

```rust
// crates/hoh-greet-tool/src/lib.rs
use hoh_kernel::{plugin, Context, Error, Inject, Service};
use hoh_tools::{define_tool, ExecuteRequest, ToolCall, Tools, ToolsResult};

#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
}

#[plugin(GreetTool, name = "greet-tool")]
pub async fn apply(ctx: Context, deps: Deps) -> Result<(), Error> {
    // Метод сервиса получает контекст вызывающего: регистрация становится эффектом этого плагина,
    // и его выгрузка убирает инструмент. В образце то же делает Proxy на `ctx.tools`.
    deps.tools.register(&ctx, define_tool(/* name: "greet", parameters, output, execute */))?;

    // `void (async () => { ... })()` образца: фоновая задача, принадлежащая плагину.
    let tools = deps.tools.clone();
    let log = ctx.logger();
    ctx.spawn("greet-tool demo call", move |cancel| async move {
        let request = ExecuteRequest {
            calls: vec![ToolCall::new("demo-1", "greet", r#"{"name":"Cordis"}"#)],
            signal: AbortSignal::from_token(cancel), // `signal` образца: отмена с причиной (раздел 10)
        };
        let result = tools.execute(request).await?;
        log.info(format_args!("tool replied: {}", result.results[0].content_json()));
        Ok(())
    })?;
    Ok(())
}

// crates/hoh-tool-logger/src/lib.rs
#[plugin(ToolLogger, name = "tool-logger")]
pub async fn apply_logger(ctx: Context, _deps: Deps) -> Result<(), Error> {
    let log = ctx.logger();
    ctx.on::<ToolsResult>(move |event| {
        log.info(format_args!("[tool-logger] {} -> {}", event.exec.name, event.result.text()));
    })?;
    Ok(())
}
```

В `hoh-tools` событие объявлено один раз, и имя с режимом берутся оттуда:

```rust
pub struct ToolsResult { pub exec: Arc<ToolExecution>, pub result: Arc<ToolExecutionResult> }
hoh_kernel::event!(ToolsResult = "tools/result", Emit);
```

Строки из образца читаются как есть: `@deepseek-ai/dsh-system-prompt` и `@deepseek-ai/dsh-tools` находятся по
псевдонимам.

## 7. TypeScript в dsh → Rust в HoH

Главное отличие. В Cordis сервис, полученный как `ctx.tools`, внутри своих методов видит `this.ctx`
**вызывающего** плагина (Proxy; service.spec «traceable effect»). Поэтому `ctx.tools.register(...)` создаёт
эффект у вызывающего. В Rust такого механизма нет: метод сервиса, который создаёт эффект, слушателя, дочерний
плагин или читает область агента от имени вызывающего, получает `ctx: &Context` первым параметром.

| В dsh | В HoH |
|---|---|
| `declare module { interface Context { tools: ToolsService } }` и `class ToolsService extends Service { constructor(ctx) { super(ctx, 'tools') } }` | `service!(pub Tools: ToolsService = "tools");` и `ctx.provide::<Tools>(Arc::new(..)).await?` в `apply` |
| сервис-шов с несколькими поставщиками (`ctx.llm`, `ctx.shell`) | `service!(pub Llm: dyn LlmApi = "llm");`, поставщики дают `Arc<dyn LlmApi>` |
| `export const inject = ['tools', 'llm']`, `static inject = [...]` | `#[derive(Inject)] struct Deps { tools: Service<Tools>, llm: Service<Llm> }` |
| `ctx.tools.execute(...)` в плагине | `deps.tools.execute(...)` |
| необязательная зависимость: `ctx.get('greeter')?.greet()` | `ctx.get::<Greeter>()` возвращает `Option`; значение не хранить (R34) |
| `ctx.inject(['x'], (ctx) => { ... })` | `ctx.inject(\|ctx, deps: XDeps\| async move { ... })?` |
| метод сервиса с `this.ctx.effect(...)` | `fn register(&self, ctx: &Context, ..) -> Result<Disposer, KernelError>` |
| `ctx.plugin(Child, config)`, `await ctx.plugin(..)` | `ctx.plugin::<Child>(json)?`, затем при необходимости `.settled().await?` |
| класс-плагин с `[Service.init]()` | тело `apply` после создания значения |
| состояние экземпляра (поля класса) | `Arc<Inner>`; изменяемое под `std::sync::Mutex` либо у задачи-актора (`PORTING.md` §6) |
| `ctx.on('tools/result', (exec, result) => ...)` | `ctx.on::<ToolsResult>(\|e\| ...)?`; событие — структура с полями-аргументами |
| `ctx.on(name, cb, { prepend: true })`, `{ global: true }` | `ctx.listen().prepend().on::<E>(..)`, `ctx.listen().global().on::<E>(..)` |
| объявление события в `interface Events` | структура и `event!(E = "имя", Режим)` в crate-владельце |
| `ctx.emit(name, a, b)` | `ctx.emit(&E { a, b })` |
| `await ctx.parallel(..)`, `ctx.serial(..)`, `ctx.bail(..)` | `ctx.parallel(E {..}).await?`, `ctx.serial(..).await?`, `ctx.bail(&E {..})` |
| `ctx.waterfall(name, args, next)`; слушатель `(args, next) => next()` | `ctx.waterfall(E {..}, \|e\| async move { .. }).await?`; слушатель `\|e, next\| async move { next.run().await }` |
| асинхронная работа в слушателе `emit` | внутри слушателя `ctx.spawn(..)` |
| `emit(thisArg, ...)` с `Context.filter`, `ctx.extend({ [kScope]: key })` | `ctx.with_filter(..).emit(..)`, `ctx.with_tag(ScopeKey(..))` и `ctx.tag::<ScopeKey>()` |
| `ctx.effect(() => { map.set(k, v); return () => map.delete(k) }, 'label')` | `ctx.effect("label", \|\| { map.insert(k, v); Undo::sync(move \|\| { map.remove(&k); }) })?` |
| `ctx.effect(function* () { yield a(); yield ctx.on(..) })` | `ctx.try_effect("label", \|u\| { u.push(a()?)?; u.push(ctx.on::<E>(..)?)?; Ok(()) })?` |
| `ctx.effect(async () => { const s = await listen(); return () => s.close() })` | `ctx.effect_async("label", \|u\| async move { let s = listen().await?; u.push(Undo::future(move \|\| async move { s.close().await }))?; Ok(()) }).await?` |
| функция-отмена `() => void`, возвращаемая вызывающему | `Disposer` |
| асинхронное освобождение: `return async () => { await x.close() }`, `Symbol.asyncDispose` | `Undo::future(..)`; ядро дожидается. В `Drop` ничего асинхронного |
| `setTimeout`/`setInterval` внутри эффекта | `ctx.spawn("label", \|cancel\| async move { .. })` с `tokio::time` внутри и выходом по `cancel` |
| `ctx.timeout(ms)`, `for await (const _ of ctx.interval(ms))` | `ctx.sleep(d).await?`, `let mut ticks = pin!(ctx.interval(d)); while ticks.next().await.is_some() { .. }` |
| `ctx.logger.info('loaded %s', name)`, `ctx.logger('x')` | `ctx.logger().info(format_args!("loaded {name}"))`, `ctx.logger().named("x")` |
| `AbortSignal` в параметрах; `signal.aborted`; `signal.reason`; `signal.throwIfAborted()`; `signal.addEventListener('abort', ..)` | `hoh_timeout::AbortSignal` (отмена с причиной, `PORTING.md` §6): `aborted()`, `reason()`, `throw_if_aborted()?`, `tokio::select!` с `cancelled()` |
| `new AbortController()`, живущий вместе с плагином | `AbortSignal::from_token(ctx.token())`; собственный контроллер — `hoh_timeout::AbortController`, связанный с `ctx.token()` |
| `ctx.on('internal/plugin', f => f === ctx.fiber && f.uid === null && abort())` | `ctx.token().cancelled().await` в `select!` |
| `AbortSignal.timeout(ms)`, `AbortSignal.any([a, b])` | `AbortSignal::timeout(..)`, `AbortSignal::any([a, b])` |
| `AsyncIterable<T>`, `async function*` | `impl Stream<Item = Result<T, E>> + Send`; источник: `futures_util::stream::unfold` либо задача `ctx.spawn` с `tokio::sync::mpsc` |
| `for await (const x of it)`, досрочный `break` | `while let Some(x) = stream.next().await`; уронить поток |
| схема `z.object({...})`, `static Config` | структура с `Deserialize`, `Serialize`, `JsonSchema`; `type Config` плагина |
| `.default(v)` | `#[serde(default)]` или `#[serde(default = "fn")]` |
| `.required()` | поле без умолчания и не `Option` |
| `z.natural()`, `.step(1)`, `.min(0)` | `u32`/`u64`; `.min(1)` целого — `NonZeroU32` |
| `.min(a).max(b)` | `#[schemars(range(min = a, max = b))]`; для длины `length(..)` |
| `.pattern(re)` | `#[schemars(regex(pattern = r".."))]`; диалект `regex`, см. `PORTING.md` §4 |
| `z.union([z.const('a'), z.const('b')])` | `enum` с `#[serde(rename_all = ..)]` |
| `z.dict(T)` | `IndexMap<String, T>` |
| `.role('secret')`, `.description(..)` | `#[schemars(extend("x-role" = "secret"))]`; doc-комментарий поля |
| `.volatile()`, чтение `config.field.value` | `Volatile<T>`; чтение `cfg.get().field` в момент использования |
| `ctx.on('loader/volatile-update', cb)` | `ctx.on::<VolatileUpdate>(..)?` |
| `ctx.on('internal/config', (fiber, next) => { проверка кандидата })` | `ctx.on_candidate::<C>(\|c\| ..)?` |
| `isolate` в строке конфигурации | без изменений в YAML; в коде `ctx.isolate("имя", Realm::new())` |
| `ctx.fiber.entry?.options.id` | `ctx.fiber().entry_id()` |
| `ctx.fiber.assertActive()` | `ctx.ensure_active()?` |
| `ctx.fiber.await()` снаружи fiber | `fiber.settled().await`; изнутри своего `apply` нельзя (R60) |
| `ctx.reflect.store`, `ctx.loader.entries()` | `Kernel::snapshot()`, `Loader::entries()` |

## 8. План тестов

Каждый файл образца переносится в файл с тем же именем по `PORTING.md` §10. Случай, названный ниже
«не переносится», записывается в `PORT_NOTES.md` crate с этой причиной. Тесты ядра работают на
`#[tokio::test(start_paused = true)]`. Столбец «Случаев» — число вызовов `it`; `it.each` посчитан за один.

### 8.1. Upstream: `[U] tests/*.spec.ts`

| Файл | Случаев | Модуль | Не переносится и почему |
|---|---:|---|---|
| `component.spec.ts` | 11 | `hoh-kernel/tests/component.rs` | «apply invalid component» (проверка типов времени исполнения); «context inspect» (`util.inspect`); «ctx.registry» (API `Map`). Формы функции, объекта и `ctx.plugin()` сводятся к одной. |
| `dispose.spec.ts` | 15 | `hoh-kernel/tests/dispose.rs` | «async yield invalid effect» (не-функция из генератора: в Rust исключено типом). Генераторы переносятся на `try_effect` и `effect_async`. |
| `events.spec.ts` | 31 | `hoh-kernel/tests/events.rs` | имена-`Symbol`, 3 случая; «treats prototype property names as ordinary events»; `once`, 4 случая (в dsh не используется); устаревший `dispatch()`, 2 случая; повторный `next()`, 3 случая (не компилируется, см. 8.4); «global listeners are not tied to a child fiber reload» (глобальных обработчиков обновления нет). |
| `fiber.spec.ts` | 22 | `hoh-kernel/tests/fiber.rs` | Всё переносится. «wrapped fiber» означает обёртку реестра; у нас это обычный `Fiber`. |
| `update.spec.ts` | 10 | `hoh-kernel/tests/update.rs` | Всё переносится. |
| `service.spec.ts` | 10 | `hoh-kernel/tests/service.rs` | `Service[Symbol.hasInstance]`, 2 случая. «traceable effect» переносится с явным `ctx` (раздел 7). |
| `service-dispose.spec.ts` | 17 | `hoh-kernel/tests/service_dispose.rs` | Всё переносится. |
| `isolate.spec.ts` | 6 | `hoh-kernel/tests/isolate.rs` | Всё переносится. |
| `intercept.spec.ts` | 7 | `hoh-kernel/tests/intercept.rs` | Остаётся «merge context intercepts from outer to inner». Параметры из объявления `inject`, 5 случаев, и «custom merge»: в dsh не используются (O5). |
| `reflect.spec.ts` | 6 | `hoh-kernel/tests/reflect.rs` | «Context.is()» (тождество между копиями библиотеки). «access check», «access error» становятся проверками компиляции (8.4). |
| `logger.spec.ts` | 11 | `hoh-kernel/tests/logger.rs` | Имя журнала от вызывающего через Proxy, 4 случая; взамен один случай: сервис пишет через собственный контекст под именем своего fiber. |
| `utils.spec.ts` | 6 | — | `DisposableList` — внутренняя структура образца. Её наблюдаемое поведение покрыто R18, R19, R41. |
| `associate.spec.ts` | 6 | — | Связанные типы и свойства через Proxy. |
| `decorator.spec.ts` | 1 | — | Декоратор `@Inject`; его место занял `#[derive(Inject)]` (8.4). |
| `invoke.spec.ts` | 2 | — | Сервис, вызываемый как функция. |
| `shadow.spec.ts` | 8 | — | Теневой контекст и отслеживание вызывающего через Proxy. Случай «resolves same-name own provisions at their definition sites» покрыт `isolate.rs`. |
| `replace.spec.ts` | 16 | — | Замена кода на лету (решение 2). |

### 8.2. dsh: `[D] scripts/*.spec.ts`

В плане названо 14 файлов: это 13 файлов `cordis-*` и `loader-*` плюс `volatile-config.spec.ts`.

| Файл | Случаев | Модуль | Не переносится и почему |
|---|---:|---|---|
| `cordis-config-access.spec.ts` | 4 | `hoh-kernel/tests/config_access.rs` | Всё переносится; «registry wrappers» — обычный `Fiber`. |
| `cordis-core-lifecycle.spec.ts` | 8 | `hoh-kernel/tests/core_lifecycle.rs` | «supports symbol events and rejects repeated waterfall delegation». |
| `cordis-core-rc12.spec.ts` | 9 | `hoh-kernel/tests/core_rc12.rs` | «merges injected metadata under context intercepts», «merges through an overridden mergeIntercept» (O5). |
| `cordis-timer.spec.ts` | 4 | `hoh-kernel/tests/timer.rs` | «completes concurrent interval reads in order» (у `Stream` один читатель); `throttle` и `debounce`, 2 случая: единственный потребитель в dsh — `boot/hmr` (O5). |
| `volatile-config.spec.ts` | 10 | `hoh-kernel/tests/volatile_config.rs` | Заморозка объектов, ленивые и сериализованные узлы схемы schemastery. Остаются умолчания, метка в схеме, сравнение. |
| `loader-config-diff.spec.ts` | 11 | `hoh-kernel/tests/config_diff.rs` | Части про «дырявые» массивы и свойства прототипа `Object`. |
| `loader-volatile-update.spec.ts` | 25 | `hoh-loader/tests/volatile_update.rs` | Экземпляры классов и циклы в конфигурации, 2 случая; сравнение `URL`; `undefined` внутри массива; сохранение в файл в «reloads real YAML and persists plain values…» (O4). |
| `cordis-entry-yaml.spec.ts` | 5 | `hoh-loader/tests/entry_yaml.rs` | «folds long scalars at the requested line width» (оформление js-yaml). |
| `cordis-entry-commit.spec.ts` | 6 | `hoh-loader/tests/entry_commit.rs` | Перемещение строки и «reports explicit Fiber saves…» (O4). Остаются id при создании и удалении, V27, оба случая про патчи. |
| `cordis-include-journal.spec.ts` | 10 | — | Обратная запись в файл не переносится (O4). |
| `cordis-loader-resolve.spec.ts` | 5 | — | Разрешение модулей Node. |
| `cordis-vendor-hmr-include.spec.ts` | 1 | — | HMR. |
| `cordis-core-api.spec.ts` | 2 | — | Генератор документации. |
| `cordis-config-files.spec.ts` | 1 | — | Служебный обход репозитория. |

### 8.3. Тесты пакетов, вошедших в `hoh-loader`

Эти файлы не читались; распределение сделано по именам и уточняется при переносе. Источник:
`[D] packages/boot/app-boot/tests`, `packages/boot/cmdline/tests`.

| Файлы | Модуль или причина |
|---|---|
| `app-boot`, `config-dump`, `config-reload`, `user-patches`, `profile`, `bundle-platform`, `startup-diagnostics`, `profile-sanitize`, `config-pattern` | `hoh-loader/tests/` с теми же именами |
| `cmdline.spec.ts` | `hoh-loader/tests/cmdline.rs` |
| `config-schema`, `schemastery-json-schema`, `loader-shape.compat` | Проекция schemastery в JSON Schema заменена `schemars`; случаи про схему состава переносятся в `hoh-loader/tests/config_schema.rs` |
| `plugin-compatibility`, `profile-compatibility`, `compatibility-preflight`, `profile-plugins`, `package-meta`, `profile-resolution*` (3 файла), `linked-resolution-matrix` | Установка и совместимость пакетов npm, разрешение модулей: не переносится |

### 8.4. Собственные тесты

- `hoh-kernel-macros/tests/ui/*.rs` на `trybuild`, обязаны не компилироваться: поле `Deps` не `Service<K>`;
  `ctx.emit` для события другого режима; второй `next.run()`; второй `disposer.dispose()`; создание
  `Service<K>` в обход `Deps` и `ctx.get`.
- `hoh-loader/tests/expr.rs`: каждое выражение из раздела 5.5 разбирается; таблица значений для заданной
  `EvalEnv`; текст вне грамматики даёт ошибку с позицией.
- `hoh-loader/tests/shipped_yaml.rs`: файлы `packages/bundle/*/cordis.patch.yml` и `web-app/presets/*.patch.yml`
  образца, скопированные в `hoh-loader/tests/fixtures/dsh/`, читаются и складываются в состав без
  предупреждений L10.
- `hoh-kernel/tests/concurrency.rs`: R59–R62, R17 (контекст прошлой активации), R23, R35.

## 9. Открытые вопросы и риски

Отступления от таблицы 6.1 исследования:

1. Запись сервиса адресуется (имя, область) с проверкой `TypeId`, а не (`TypeId`, область). Строки конфигурации
   называют сервисы строками (`inject`, `isolate`, выражения), и имя обязано остаться именем dsh.
2. Событие — сам тип-сообщение с типом режима, а не `trait Event { type Args; type Output; const MODE }`.
   Режим в типе делает вызов `ctx.emit` для события `Parallel` ошибкой компиляции.
3. Задача ядра — обычная задача многопоточного runtime, а не `LocalSet` и не отдельный поток: состояние ядра
   `Send`, а выделенный поток усложнил бы тесты с остановленным временем.
4. Таблица плагинов явная (`PluginRegistry::add`), без `inventory` и `linkme`: `PORTING.md` §6 запрещает
   `static` с реестрами, а явный список читается поиском.
5. Контекст привязан к активации (R17). Образец проверяет только состояние fiber.

Вопросы. У каждого названо рекомендуемое решение; документ написан в расчёте на него.

- **O1. Проверка конфигурации.** Собственный обход закрытого набора слов схемы (R48, около 250 строк) или
  crate `jsonschema`. Рекомендация: собственный. `jsonschema` 0.61.2 тянет 18 обязательных прямых
  зависимостей помимо serde, среди них вторую версию `getrandom`, а тексты отказов всё равно пришлось бы
  переводить в вид schemastery (R49).
- **O2. Журнал.** Собственный журнал ядра (перенос `logger.ts`) или `tracing`. Рекомендация: собственный. Он
  принадлежит экземпляру ядра, и тесты читают `buffer()`; у `tracing` подписчик глобальный либо потоковый, что
  ломает параллельные тесты с задачами. Мост в `tracing` для сторонних библиотек добавит приложение.
- **O3. Смена `isolate`, `intercept`, `inject` у живой строки.** Пересоздание строки вместо замены областей на
  месте (L13). Рекомендация: пересоздание. Цена: потребители и сама строка перезапускаются, даже когда
  набор значений не изменился. В поставке `isolate` стоит на группах пресетов и сценариев, `intercept` и
  объектная форма `inject` не встречаются.
- **O4. Обратная запись состава в файл** (`[D] vendor/include/src/journal.ts`, V14, V26). Рекомендация: не
  переносить. Запись запускают `fiber.update()` без `noSave` и правки дерева внутри включения. В пакетах dsh
  (без `client` и `experimental`) вызовов `fiber.update` и `fiber.restart` нет; `loader.create` и `remove`
  зовут `app-boot` и `directory-picker-auto`, остальное приходится на переделываемый `plugin-manager`.
  Редактор конфигурации пишет файл патча сам и вызывает перекомпоновку. Следствие: из 14 файлов плана
  `cordis-include-journal.spec.ts` (10 случаев) выпадает. Нужно решение ведущего.
- **O5. Что не взято как не используемое в dsh:** `once`; `throttle` и `debounce`; параметры перехвата в
  объявлении `inject` и собственная функция слияния; `Service.check`; глобальные обработчики `internal/update`;
  события `internal/get`, `set`, `listener`, `commit`, `replace`; перемещение строк между группами.
  Рекомендация: не брать; каждое добавляется позже без изменения остального API. Решение 7 («как в Cordis»)
  выполнено для `isolate` полностью, для `intercept` — в части слоёв контекста.
- **O6. Профиль и пакеты.** dsh хранит состав профиля в `package.json` (`dsh.profile.bundles`), а пакеты
  патчей — в пакетах npm (`dsh.bundle.patch`). Рекомендация для этапа 0: читать профиль dsh как есть, пакеты
  получать через `BundleSource`; встроенные пакеты — YAML образца, вшитый в исполняемый файл. Свой формат
  профиля (TOML) можно добавить позже вторым читателем. Установка пакетов, проверка версий и разрешение
  модулей из `app-boot` в `hoh-loader` не входят: это область переделываемого `plugin-manager`.
- **O7. Данные сервисов для выражений.** `ServiceKey::view` против списка фактов, которые запускающий код
  передаёт загрузчику. Рекомендация: `view`. Сервисов три, и два из них (`headlessStartup`, `webStartup`)
  предоставляют плагины, которых строки ждут через `inject`.
- **O8. `#[plugin]`.** Макрос только сокращает запись `impl Plugin`. Рекомендация: оставить (он в утверждённом
  списке символов) и разрешить обе записи; в заданиях исполнителям назвать одну основной — макрос.
- **O9. Размещение `logger-console`.** В описи он отнесён к `hoh-kernel`; так и описано (`ConsoleExporter`).
  Если ведущий захочет держать stderr вне ядра, плагин переносится в `apps/hoh` без изменения API.
- **O10. Слежение за файлом патча.** В dsh за файлом профиля следит `boot/hmr`, который не переносится.
  Рекомендация: в этапе 0 перекомпоновку вызывает только редактор конфигурации. Для слежения понадобится
  `notify` 8.2.2, а его лицензия CC0-1.0 не входит в разрешённый список: нужно отдельное решение.

Риски:

- **Зависание на ожидании себя.** R60 ловит ожидание из задачи `apply`. Ожидание из задачи, порождённой
  плагином, ядро не распознает. Ревьюер проверяет: внутри плагина нет `.await` на `dispose`, `restart`,
  `update`, `settled` собственного fiber и предков.
- **Задержка опубликованной таблицы.** `Fiber::state()` и `ctx.get` читают состояние на конец последнего хода
  ядра. Код, которому нужен точный момент, ждёт `settled()` или слушает `InternalStatus`.
- **Тексты отказов конфигурации.** JavaScript печатает значения в `but got …` по своим правилам (R49).
  Тесты пакетов, сверяющие такие тексты целиком, придётся разбирать по одному.
- **Диалект регулярных выражений** в `pattern` (7 мест в dsh): `regex` против JavaScript, см. `PORTING.md` §4.
- **Слушатели `Emit` с долгой работой** блокируют рассылающего, как и в образце. Асинхронная работа уходит в
  `ctx.spawn`; число таких задач ничем не ограничено.
- **Циклы `Arc`** между сервисами дают ложное предупреждение R35 и настоящую утечку памяти. Сервис хранит
  чужой сервис только как `Service<K>` из `Deps`, а обратные ссылки делает `Weak`.
- **Переходные лицензии.** Проверены только прямые зависимости (`DEPS.md`). Дерево целиком проверяется после
  первой сборки командой `cargo tree` и средством проверки лицензий, которое выберет ведущий.

## 10. Решения ведущего (2026-11-16)

Документ принят как основание этапа 0. По открытым вопросам:

| Вопрос | Решение |
|---|---|
| O1 | Собственная проверка схемы (R48), без `jsonschema`. |
| O2 | Собственный журнал ядра, без `tracing`. |
| O3 | Смена `isolate`, `intercept`, `inject` у живой строки пересоздаёт строку. |
| O4 | Обратная запись состава в файл не переносится. `cordis-include-journal.spec.ts` не переносится; об этом сообщается основателю как об уточнении плана. |
| O5 | Перечисленное не берём; добавляется позже по первому настоящему потребителю. |
| O6 | Профиль dsh читается как есть; встроенные пакеты патчей вшиты в исполняемый файл; установка пакетов и совместимость npm в `hoh-loader` не входят. |
| O7 | `ServiceKey::view`. |
| O8 | Обе записи плагина разрешены; в заданиях основной названа `#[plugin]`. |
| O9 | `logger-console` остаётся в `hoh-kernel`. |
| O10 | В этапе 0 за файлом патча никто не следит; перекомпоновку вызывает редактор конфигурации. Лицензия CC0-1.0 допустима для закрытого продукта (условий не накладывает), `notify` добавляется, когда появится потребитель. |

Дополнения к документу:

- **Отмена с причиной.** `ctx.token()` остаётся `CancellationToken`: это отмена активации, причины у неё нет.
  Всё, что в образце принимает или создаёт `AbortSignal`, переносится типом `hoh_timeout::AbortSignal`: образец
  читает `signal.reason` в 93 местах и сравнивает причину по тождеству. Таблица раздела 7 исправлена.
- **Тип ошибки.** `hoh_kernel::Error` и `hoh_error::BoxError` — один и тот же тип; в коде плагинов пишется
  `BoxError`. Ошибки с кодом — `hoh_error::HarnessError` (`PORTING.md` §7).
- **Изменяемые аргументы события.** Слушатель получает `&E` или `Arc<E>`. Если в образце слушатели изменяют
  аргументы события (конвейеры `waterfall` инструментов), изменяемая часть события лежит в поле с
  `std::sync::Mutex`; блокировка не удерживается через `.await` и через `next.run()`.
