use hoh_loader::Expr;
#[test]
fn every_expression_in_section_5_5_parses() {
    let sources = include_str!("fixtures/expressions.txt");
    assert_eq!(sources.lines().count(), 76);
    for source in sources.lines() {
        let source = source
            .replace("E.", "process.env.")
            .replace("P.", "process.getBuiltinModule('node:path').")
            .replace(
                "REQ(",
                "process.getBuiltinModule('node:module').createRequire(baseUrl).resolve(",
            );
        Expr::parse(&source).unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}
#[test]
fn text_outside_the_grammar_fails_with_its_position() {
    for source in [
        "",
        "process.exit()",
        "process.env['X']",
        "ctx.get('s').field",
        "ctx.s",
        "new Date()",
        "1 * 2",
        "a => a",
        "[1,]",
        "JSON.stringify(1)",
        "process.cwd() // comment",
        "true === true === true",
        "process.getBuiltinModule('node:fs').readFile('x')",
        "dshHomePath(process.env.X)",
        "process.getBuiltinModule('node:path').dirname(process.getBuiltinModule('node:module').createRequire(baseUrl).resolve('p/other.json'))",
    ] {
        let e = Expr::parse(source).unwrap_err();
        assert!(e.position <= source.encode_utf16().count(), "{source}");
        assert!(e.line >= 1 && e.column >= 1);
    }
    let e = Expr::parse("true\n  + wrong").unwrap_err();
    assert_eq!((e.line, e.column, e.position), (2, 5, 9));
    let e = Expr::parse("'😀' + wrong").unwrap_err();
    assert_eq!(e.position, 7);
}
#[test]
fn escaped_strings_and_operator_precedence_parse() {
    for source in [
        r#"'it\'s' + "a\u0020b""#,
        r#"'\uD83D\uDE00'"#,
        "false ? 1 : true ? 2 : 3",
        "1 + 2 === '12' && !false || undefined ?? 'x'",
        "[true,false,undefined].includes(null)",
    ] {
        Expr::parse(source).unwrap();
    }
}
