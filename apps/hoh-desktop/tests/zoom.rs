use hoh_desktop::zoom::{Zoom, factor, load, next_level, save};

#[test]
fn steps_and_limits_are_those_of_desktop_t3() {
    assert_eq!(next_level(0.0, Zoom::In), 0.5);
    assert_eq!(next_level(0.5, Zoom::In), 1.0);
    assert_eq!(next_level(1.0, Zoom::Out), 0.5);
    assert_eq!(next_level(3.5, Zoom::Reset), 0.0);
    assert!((factor(1.0) - 1.2).abs() < 1e-9 && factor(0.0) == 1.0);
    let mut level = 0.0;
    for _ in 0..100 {
        level = next_level(level, Zoom::In);
    }
    assert!((factor(level) - 5.0).abs() < 1e-9);
    for _ in 0..200 {
        level = next_level(level, Zoom::Out);
    }
    assert!((factor(level) - 0.25).abs() < 1e-9);
}

#[test]
fn the_level_is_kept_between_launches() {
    let dir = std::path::PathBuf::from(std::env::var("TMPDIR").unwrap_or("/dev/shm".into()))
        .join(format!("hoh-zoom-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(load(&dir), 0.0);
    save(&dir, 1.0);
    assert_eq!(load(&dir), 1.0);
    std::fs::write(dir.join("zoom-level"), "garbage").unwrap();
    assert_eq!(load(&dir), 0.0);
    std::fs::write(dir.join("zoom-level"), "99").unwrap();
    assert!(factor(load(&dir)) <= 5.0);
    std::fs::remove_dir_all(dir).unwrap();
}
