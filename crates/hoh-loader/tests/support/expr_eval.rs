use super::*;
use std::sync::Arc;
fn env() -> EvalEnv {
    EvalEnv {
        home: "/test/home".into(),
        cwd: "/test/work".into(),
        exe: "/bin/hoh".into(),
        platform: "linux",
        var: Arc::new(|name| match name {
            "EMPTY" => Some("".into()),
            "OFF" => Some("0".into()),
            "OBJECT" => Some("{\"x\":1}".into()),
            _ => None,
        }),
    }
}
#[test]
fn values_use_javascript_operands_truthiness_and_service_views() {
    let cases = [
        ("process.env.EMPTY ?? 'default'", Some(json!(""))),
        ("process.env.EMPTY || 'default'", Some(json!("default"))),
        ("process.env.OFF && 200 || 12500", Some(json!(200))),
        ("process.env.MISSING && 200 || 12500", Some(json!(12500))),
        ("false && ctx.absent.x", Some(json!(false))),
        ("true || ctx.absent.x", Some(json!(true))),
        ("undefined ?? null", Some(Value::Null)),
        ("undefined", None),
        ("![]", Some(json!(false))),
        ("!ctx.get('absent')", Some(json!(true))),
        ("ctx.get('empty')?.name", None),
        ("ctx.get('absent')?.a.b", None),
        (
            "ctx.get('profileContext')?.name !== 'desktop'",
            Some(json!(false)),
        ),
        ("ctx.headlessStartup.task", Some(json!("run"))),
        ("ctx.webStartup.port ?? 3080", Some(json!(3080))),
        ("[1, '1', undefined].includes(undefined)", Some(json!(true))),
        ("[1].includes('1')", Some(json!(false))),
        ("[null].includes(undefined)", Some(json!(false))),
        ("1 === 1.0", Some(json!(true))),
        ("null === undefined", Some(json!(false))),
        ("[] === []", Some(json!(false))),
        ("Number(' 0x10 ')", Some(json!(16))),
        ("Number('')", Some(json!(0))),
        ("Number('1e2')", Some(json!(100))),
        ("JSON.parse(process.env.OBJECT)", Some(json!({"x":1}))),
        (
            "process.cwd() + '/skills'",
            Some(json!("/test/work/skills")),
        ),
        ("1 + true + null", Some(json!("1truenull"))),
        (
            "dshHomePath('office-kit','outputs')",
            Some(json!("/test/home/office-kit/outputs")),
        ),
        (
            "process.getBuiltinModule('node:path').join(process.cwd(),'a','../b')",
            Some(json!("/test/work/b")),
        ),
        (
            "process.getBuiltinModule('node:path').resolve('../b')",
            Some(json!("/test/b")),
        ),
        (
            "process.getBuiltinModule('node:path').join(process.getBuiltinModule('node:path').dirname(process.getBuiltinModule('node:module').createRequire(baseUrl).resolve('@dsh/p/package.json')), 'skills')",
            Some(json!("/resources/p/skills")),
        ),
        (
            "decodeURIComponent(new URL('./runner.mjs', 'file://' + '/test/a%20b/config.yml').pathname)",
            Some(json!("/test/a b/runner.mjs")),
        ),
        (
            "process.platform === 'linux' ? process.execPath : process.cwd()",
            Some(json!("/bin/hoh")),
        ),
    ];
    for (text, want) in cases {
        let parsed = Expr::parse(text).unwrap();
        let result = evaluate(
            &parsed,
            &env(),
            Path::new("/base"),
            &|name| match name {
                "profileContext" => Some(json!({"name":"desktop"})),
                "headlessStartup" => Some(json!({"task":"run"})),
                "empty" | "webStartup" => Some(json!({})),
                _ => None,
            },
            &|name| (name == "@dsh/p").then(|| PathBuf::from("/resources/p")),
        )
        .unwrap();
        assert_eq!(result, want, "{text}");
    }
}
#[test]
fn evaluation_errors_are_explicit() {
    for source in [
        "ctx.absent.x",
        "ctx.get('empty')?.missing.deep",
        "undefined + 'x'",
        "Number('no')",
        "Number(null)",
        "JSON.parse('invalid')",
        "process.getBuiltinModule('node:path').dirname(process.getBuiltinModule('node:module').createRequire(baseUrl).resolve('absent/package.json'))",
        "decodeURIComponent(new URL('./x', 'file:///bad%XY/config').pathname)",
    ] {
        assert!(
            evaluate(
                &Expr::parse(source).unwrap(),
                &env(),
                Path::new("/base"),
                &|s| (s == "empty").then(|| json!({})),
                &|_| None
            )
            .is_err(),
            "{source}"
        );
    }
}

#[test]
fn every_catalogue_expression_evaluates_with_supplied_environment_and_lookups() {
    let mut environment = env();
    environment.var = Arc::new(|name| match name {
        "DSH_SNAPSHOT_FILE" => Some("/snapshots/config.yml".into()),
        "DSH_SNAPSHOT_SPILL_ROOT" => Some("/spill".into()),
        "DSH_TEST_CHILD_PATCHES" => Some("[]".into()),
        _ => None,
    });
    for source in include_str!("../fixtures/expressions.txt").lines() {
        let source = source
            .replace("E.", "process.env.")
            .replace("P.", "process.getBuiltinModule('node:path').")
            .replace(
                "REQ(",
                "process.getBuiltinModule('node:module').createRequire(baseUrl).resolve(",
            );
        let parsed = Expr::parse(&source).unwrap();
        evaluate(&parsed,&environment,Path::new("/base"),&|name| match name {
            "profileContext"=>Some(json!({"name":"desktop"})),
            "headlessStartup"=>Some(json!({"task":"run","sessionId":"s1","json":false,"additionalWritableRoots":[]})),
            "webStartup"=>Some(json!({"trustedHosts":[],"tls":false,"publicUrl":null,"openBrowser":false,"port":3080,"host":"127.0.0.1"})),_=>None,
        },&|name| (name=="@deepseek-ai/dsh-agent-preset").then(||PathBuf::from("/resources/preset"))).unwrap_or_else(|e|panic!("{source}: {e}"));
    }
}
#[test]
fn path_semantics_cover_posix_and_windows_without_reading_the_host() {
    for (platform, cwd, home, cases) in [
        (
            "linux",
            "/work/project",
            "/home/test",
            vec![
                (
                    "process.getBuiltinModule('node:path').resolve('x/')",
                    "/work/project/x",
                ),
                (
                    "process.getBuiltinModule('node:path').join('/a','/b','..','c/')",
                    "/a/c/",
                ),
                ("dshHomePath('a','../b')", "/home/test/b"),
            ],
        ),
        (
            "win32",
            r"C:\work\project",
            r"C:\Users\test",
            vec![
                (
                    "process.getBuiltinModule('node:path').resolve('x/')",
                    r"C:\work\project\x",
                ),
                (
                    "process.getBuiltinModule('node:path').resolve('/x')",
                    r"C:\x",
                ),
                (
                    "process.getBuiltinModule('node:path').resolve('C:../x')",
                    r"C:\work\x",
                ),
                (
                    "process.getBuiltinModule('node:path').resolve('D:/a/../b/')",
                    r"D:\b",
                ),
                (
                    "process.getBuiltinModule('node:path').join('C:/a','/b','..','c/')",
                    r"C:\a\c\",
                ),
                ("dshHomePath('a','../b')", r"C:\Users\test\b"),
                (
                    "process.getBuiltinModule('node:path').join('//server/share', 'x')",
                    r"\\server\share\x",
                ),
            ],
        ),
    ] {
        let mut environment = env();
        environment.platform = platform;
        environment.cwd = cwd.into();
        environment.home = home.into();
        for (source, want) in cases {
            assert_eq!(
                evaluate(
                    &Expr::parse(source).unwrap(),
                    &environment,
                    Path::new("/base"),
                    &|_| None,
                    &|_| None
                )
                .unwrap(),
                Some(json!(want)),
                "{platform}: {source}"
            );
        }
    }
}
#[test]
fn json_parse_uses_javascript_numbers_and_optional_chaining_keeps_null_distinct() {
    for (source, want) in [
        (
            "JSON.parse('[1.0,9007199254740993]')",
            Some(json!([1, 9007199254740992u64])),
        ),
        ("ctx.get('s')", Some(Value::Null)),
        ("ctx.get('s')?.x", None),
        ("[undefined, null]", Some(json!([null, null]))),
    ] {
        assert_eq!(
            evaluate(
                &Expr::parse(source).unwrap(),
                &env(),
                Path::new("/base"),
                &|_| Some(Value::Null),
                &|_| None
            )
            .unwrap(),
            want,
            "{source}"
        );
    }
}
