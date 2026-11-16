//! Aspect-preserving request-image geometry shared by model routes and pricing.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedDimensions {
    pub width: u64,
    pub height: u64,
}

/// Positive source dimensions and pixel budget; never enlarges a small source.
pub fn request_image_dimensions(width: u64, height: u64, max_pixels: u64) -> ProjectedDimensions {
    let scale = (max_pixels as f64 / (width as f64 * height as f64))
        .sqrt()
        .min(1.0);
    if scale == 1.0 {
        return ProjectedDimensions { width, height };
    }
    if width >= height {
        let mut projected_width = ((width as f64 * scale).floor() as u64).max(1);
        let mut projected_height = rounded_short_edge(projected_width, height, width);
        while projected_width as f64 * projected_height as f64 > max_pixels as f64
            && projected_width > 1
        {
            projected_width -= 1;
            projected_height = rounded_short_edge(projected_width, height, width);
        }
        return ProjectedDimensions {
            width: projected_width,
            height: projected_height,
        };
    }
    let mut projected_height = ((height as f64 * scale).floor() as u64).max(1);
    let mut projected_width = rounded_short_edge(projected_height, width, height);
    while projected_width as f64 * projected_height as f64 > max_pixels as f64
        && projected_height > 1
    {
        projected_height -= 1;
        projected_width = rounded_short_edge(projected_height, width, height);
    }
    ProjectedDimensions {
        width: projected_width,
        height: projected_height,
    }
}

/// Keep an exact positive long edge, rounding the short edge to the nearest pixel.
pub fn long_edge_dimensions(width: u64, height: u64, long_edge: u64) -> ProjectedDimensions {
    if long_edge >= width.max(height) {
        return ProjectedDimensions { width, height };
    }
    if width >= height {
        ProjectedDimensions {
            width: long_edge,
            height: rounded_short_edge(long_edge, height, width),
        }
    } else {
        ProjectedDimensions {
            width: rounded_short_edge(long_edge, width, height),
            height: long_edge,
        }
    }
}

fn rounded_short_edge(long_edge: u64, short_source: u64, long_source: u64) -> u64 {
    ((long_edge as f64 * short_source as f64 / long_source as f64).round() as u64).max(1)
}
