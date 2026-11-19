#[test]
fn parses_each_file_with_the_documented_generic_plugin_fields() {
    for (file, source, id, server, pin) in [
        (
            "memorix.cordis.yml",
            include_str!("../assets/examples/mcp-memory/memorix.cordis.yml"),
            "memory-memorix",
            "memorix",
            "1.3.0",
        ),
        (
            "mcp-reference-memory.cordis.yml",
            include_str!("../assets/examples/mcp-memory/mcp-reference-memory.cordis.yml"),
            "memory-mcp-reference",
            "reference_memory",
            "2026.7.4",
        ),
        (
            "engram.cordis.yml",
            include_str!("../assets/examples/mcp-memory/engram.cordis.yml"),
            "memory-engram",
            "engram",
            "1.20.0",
        ),
    ] {
        let patches =
            hoh_loader::parse_patches(source, &std::path::Path::new("/examples").join(file))
                .unwrap();
        assert_eq!(patches.len(), 1);
        let inserted = patches[0].insert.as_ref().unwrap();
        assert_eq!(inserted.len(), 1);
        let row = &inserted[0];
        assert_eq!(row.id, id);
        assert_eq!(row.name, "@deepseek-ai/dsh-mcp-client");
        assert_eq!(row.config["serverName"], server);
        assert_eq!(row.config["transport"], "stdio");
        assert!(source.lines().next().unwrap().contains(pin));
        assert!(!source.contains("DEEPSEEK_API_KEY"));
        for suffix in source.match_indices("sk-").map(|(i, _)| &source[i + 3..]) {
            assert!(
                suffix
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || b"_-".contains(b))
                    .count()
                    < 8
            );
        }
    }
}
