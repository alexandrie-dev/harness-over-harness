//! Page zoom of the main window, as in desktop T3 (`DesktopWindow.zoomWindow`: Ctrl+`=`/`+` and Ctrl+`-`
//! move the zoom level by 0.5, Ctrl+0 resets it; the factor is 1.2^level, as in Electron/Chromium, whose
//! range is 0.25..5). The level is kept in the window's own data folder between launches.
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

const STEP: f64 = 0.5;
const BASE: f64 = 1.2;
const MIN_FACTOR: f64 = 0.25;
const MAX_FACTOR: f64 = 5.0;

pub fn factor(level: f64) -> f64 {
    BASE.powf(level).clamp(MIN_FACTOR, MAX_FACTOR)
}

/// The level after a step; stays inside the factor range.
pub fn next_level(level: f64, direction: Zoom) -> f64 {
    let wanted = match direction {
        Zoom::Reset => 0.0,
        Zoom::In => level + STEP,
        Zoom::Out => level - STEP,
    };
    let (low, high) = (MIN_FACTOR.ln() / BASE.ln(), MAX_FACTOR.ln() / BASE.ln());
    wanted.clamp(low, high)
}

const FILE: &str = "zoom-level";

pub fn load(dir: &Path) -> f64 {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|level| level.is_finite())
        .map_or(0.0, clamp_saved)
}

fn clamp_saved(level: f64) -> f64 {
    let (low, high) = (MIN_FACTOR.ln() / BASE.ln(), MAX_FACTOR.ln() / BASE.ln());
    level.clamp(low, high)
}

pub fn save(dir: &Path, level: f64) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(FILE), format!("{level}\n"));
}
