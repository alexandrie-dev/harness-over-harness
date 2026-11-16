pub(crate) fn suggest_similar(word: &str, candidates: &[String]) -> String {
    let options = word.starts_with("--");
    let word = if options { &word[2..] } else { word };
    let a: Vec<_> = word.encode_utf16().collect();
    let mut best = 3;
    let mut similar = Vec::new();
    for candidate in candidates {
        let candidate = if options {
            candidate.get(2..).unwrap_or_default()
        } else {
            candidate
        };
        let b: Vec<_> = candidate.encode_utf16().collect();
        if b.len() <= 1 {
            continue;
        }
        let distance = edit_distance(&a, &b);
        let length = a.len().max(b.len());
        if (length.saturating_sub(distance)) as f64 / length as f64 > 0.4 {
            if distance < best {
                best = distance;
                similar.clear();
            }
            if distance == best && !similar.contains(&candidate) {
                similar.push(candidate);
            }
        }
    }
    // ASCII command and flag names used by the harness have the same order as localeCompare.
    similar.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| b.cmp(a))
    });
    let names: Vec<_> = similar
        .iter()
        .map(|s| {
            if options {
                format!("--{s}")
            } else {
                s.to_string()
            }
        })
        .collect();
    match names.len() {
        0 => String::new(),
        1 => format!("\n(Did you mean {}?)", names[0]),
        _ => format!("\n(Did you mean one of {}?)", names.join(", ")),
    }
}

fn edit_distance(a: &[u16], b: &[u16]) -> usize {
    if a.len().abs_diff(b.len()) > 3 {
        return a.len().max(b.len());
    }
    let mut d = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, value) in d[0].iter_mut().enumerate() {
        *value = j;
    }
    for j in 1..=b.len() {
        for i in 1..=a.len() {
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}
