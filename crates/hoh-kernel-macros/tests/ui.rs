#[test]
fn typed_api_and_macro_diagnostics() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/*.rs");
    tests.pass("tests/pass/*.rs");
}
