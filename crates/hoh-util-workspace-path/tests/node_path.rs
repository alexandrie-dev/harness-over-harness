use hoh_util_workspace_path::node_path;
use serde_json::{Value, json};

#[test]
fn every_node_posix_fixture_line() {
    let mut count = 0;
    for (index, line) in include_str!("fixtures/node_path.jsonl").lines().enumerate() {
        let row: Value = serde_json::from_str(line).unwrap();
        let args: Vec<_> = row["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        let actual = match row["fn"].as_str().unwrap() {
            "normalize" => json!(node_path::normalize(args[0])),
            "dirname" => json!(node_path::dirname(args[0])),
            "basename" => json!(node_path::basename(args[0])),
            "extname" => json!(node_path::extname(args[0])),
            "isAbsolute" => json!(node_path::is_absolute(args[0])),
            "join" => json!(node_path::join(&args)),
            "resolve" => json!(node_path::resolve(args[0], &args[1..])),
            "relative" => json!(node_path::relative(args[0], args[1], args[2])),
            other => panic!("unknown fixture function {other}"),
        };
        assert_eq!(actual, row["out"], "line {}: {line}", index + 1);
        count += 1;
    }
    assert_eq!(count, 8835);
}
