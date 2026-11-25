//! The launcher's flags stop at the first app argument.
use hoh_commander::{
    Command, CommanderError, InvalidArgumentError, Options, Output, ParseError, ParseOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum DshInvocation {
    Profile {
        profile: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_default_profile: Option<String>,
        patches: Vec<String>,
        args: Vec<String>,
    },
    DumpConfig {
        profile: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_default_profile: Option<String>,
        default_only: bool,
        patches: Vec<String>,
    },
    DumpConfigSchema {
        profile: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_default_profile: Option<String>,
        patches: Vec<String>,
    },
    Plugin {
        profile: String,
        args: Vec<String>,
    },
}
const HELP_EXAMPLES: &str = r#"
Примеры:
  hoh headless "Составь список файлов"
  hoh sdk-minimal --dump-config
  hoh sdk-minimal --dump-config-schema
  hoh sdk-minimal --patch ./extra.yml --dump-config
"#;
fn select_profile(value: &str, previous: Option<&Value>) -> Result<Value, InvalidArgumentError> {
    if previous.is_some() {
        return Err(InvalidArgumentError::new("Профиль можно указать только один раз"));
    }
    Ok(value.into())
}
fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
fn reject_desktop(program: &Command, profile: &str) -> Result<(), CommanderError> {
    if profile.to_lowercase() == "desktop" {
        return Err(program.error(
            "Ошибка: профиль «desktop» зарезервирован для настольного приложения",
        ));
    }
    Ok(())
}
fn resolve_boot(
    program: &Command,
    profile: String,
    options: &Options,
    args: Vec<String>,
) -> Result<DshInvocation, CommanderError> {
    let patches = strings(options.get("patch"));
    let from_default_profile = options
        .get("fromDefaultProfile")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if patches.iter().any(String::is_empty) {
        return Err(program.error("Ошибка: укажите путь для --patch"));
    }
    if from_default_profile.as_deref() == Some("") {
        return Err(program.error("Ошибка: укажите имя шаблона для --from-default-profile"));
    }
    let dump = ["dumpConfig", "dumpDefaultConfig", "dumpConfigSchema"]
        .iter()
        .filter(|k| options.get(**k) == Some(&Value::Bool(true)))
        .count();
    if dump == 0 {
        return Ok(DshInvocation::Profile {
            profile,
            from_default_profile,
            patches,
            args,
        });
    }
    if dump > 1 {
        return Err(program.error("Ошибка: выберите только один из флагов --dump-config, --dump-default-config и --dump-config-schema"));
    }
    if !args.is_empty() {
        return Err(program.error(&format!(
            "Ошибка: при выводе конфигурации нельзя передавать аргументы приложения: {}",
            args.iter()
                .map(|a| hoh_util_values::js_json_stringify(&Value::String(a.clone())))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }
    if options.get("dumpConfigSchema") == Some(&Value::Bool(true)) {
        return Ok(DshInvocation::DumpConfigSchema {
            profile,
            from_default_profile,
            patches,
        });
    }
    let default_only = options.get("dumpDefaultConfig") == Some(&Value::Bool(true));
    if default_only && !patches.is_empty() {
        return Err(program
            .error("Ошибка: --dump-default-config выводит встроенные слои и не принимает --patch"));
    }
    Ok(DshInvocation::DumpConfig {
        profile,
        from_default_profile,
        default_only,
        patches,
    })
}
/// Help/version/rejection return Commander's exit, after writing through output.
pub fn parse_dsh_args(
    argv: &[String],
    version: &str,
    manage_desktop_profile: bool,
    output: Output,
) -> Result<DshInvocation, ParseError> {
    let first = argv.first().map(String::as_str);
    let resolved = Arc::new(Mutex::new(None));
    let result = resolved.clone();
    let mut program = Command::new();
    program.name("hoh").help_headings("Использование", "Аргументы", "Параметры", "Команды")
        .version_with_flags(version,"-V, --version","Показать версию")
        .usage("[--profile] <имя> [параметры] [аргументы приложения…]")
        .description("Запуск профиля HoH и просмотр его конфигурации.")
        .add_help_text("after",HELP_EXAMPLES).exit_override().configure_output(output)
        .help_option_enabled(false).help_command(false).allow_unknown_option(true).pass_through_options(true).enable_positional_options(true)
        .argument("[аргументы...]","Аргументы, передаваемые приложению профиля")
        .option_with_parser("--profile <имя>","Имя профиля в $DSH_HOME/profiles",select_profile)
        .option("--from-default-profile <шаблон>","Пересоздать профиль из встроенного шаблона")
        .option_with_parser("--patch <путь>","Добавить патч (можно указать несколько раз)",|value,previous|{let mut items=strings(previous);items.push(value.into());Ok(items.into_iter().map(Value::String).collect::<Vec<_>>().into())})
        .option("--dump-config","Вывести итоговую конфигурацию профиля")
        .option("--dump-config-schema","Вывести JSON Schema конфигурации и патчей")
        .option("--dump-default-config","Вывести встроенную конфигурацию профиля")
        .action(move|arguments,options,program|{
            let args=strings(arguments.first().and_then(Option::as_ref));
            let Some(profile)=options.get("profile").and_then(Value::as_str)else{
                if args.iter().any(|a|matches!(a.as_str(),"-h"|"--help")){return Err(program.help().into());}
                return Err(program.error("Ошибка: укажите профиль через --profile <имя>").into());
            };
            if profile.is_empty(){return Err(program.error("Ошибка: укажите имя профиля").into());}
            reject_desktop(program,profile)?;
            *result.lock().unwrap_or_else(|e|e.into_inner())=Some(resolve_boot(program,profile.into(),options,args)?);Ok(())
        });
    if first == Some("plugin") {
        let result = resolved.clone();
        program.command("plugin").usage("[параметры] [аргументы…]").description("Управление пакетами профиля. В этой сборке команда недоступна.")
            .help_option("-h, --help","Показать справку").pass_through_options(true)
            .required_option_with_parser("--profile <имя>","Имя профиля",select_profile)
            .allow_unknown_option(true).argument("[аргументы...]","Аргументы pnpm")
            .action(move|arguments,options,program|{
                let profile=options.get("profile").and_then(Value::as_str).unwrap_or_default();
                if profile.is_empty(){return Err(program.error("Ошибка: укажите имя профиля").into());}
                if !manage_desktop_profile{reject_desktop(program,profile)?;}
                let args=strings(arguments.first().and_then(Option::as_ref));
                if args.is_empty(){return Err(program.error("Ошибка: укажите аргументы pnpm для команды plugin").into());}
                let profile=if profile.to_lowercase()=="desktop"{String::from("desktop")}else{profile.into()};
                *result.lock().unwrap_or_else(|e|e.into_inner())=Some(DshInvocation::Plugin{profile,args});Ok(())
            });
    }
    let expanded = if first.is_some_and(|f| !f.starts_with('-') && f != "plugin") {
        std::iter::once("--profile".into())
            .chain(argv.iter().cloned())
            .collect()
    } else {
        argv.to_vec()
    };
    program.parse(expanded, ParseOptions::USER)?;
    resolved
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or_else(|| ParseError::Other("HoH: не удалось разобрать команду".into()))
}
