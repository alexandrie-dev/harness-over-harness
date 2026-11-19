//! DeepSeek's published v41 vision-token calculator.

use hoh_attachment::{ProjectedDimensions, long_edge_dimensions};

const PATCH_SIZE: u64 = 14;
const DOWNSAMPLE_RATIO: u64 = 3;
const MAX_IMAGE_TOKENS: u64 = 1024;
const MIN_PIXELS: u64 = 544 * 544;
const CELL_SIZE: u64 = PATCH_SIZE * DOWNSAMPLE_RATIO;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridResize {
    grid_height: u64,
    grid_width: u64,
    best_height: u64,
    best_width: u64,
    num_tokens: u64,
}

const fn ceil_div(value: u64, divisor: u64) -> u64 {
    value.div_ceil(divisor)
}
const fn grid_tokens(height: u64, width: u64) -> u64 {
    height * (width + 1) + 2
}
const fn grid_cells(padded: u64) -> u64 {
    ceil_div(padded / PATCH_SIZE, DOWNSAMPLE_RATIO)
}

fn solve_resize_ratio(height: u64, width: u64, budget: u64) -> GridResize {
    let aspect = height as f64 / width as f64;
    let ideal_width = (((budget - 2) as f64 / aspect) + 0.25).sqrt() - 0.5;
    let ideal_height = ideal_width * aspect;
    let (best_height, best_width) = if ideal_width < 1.0 {
        (((budget - 2) / 2) * CELL_SIZE, CELL_SIZE)
    } else if ideal_height < 1.0 {
        (CELL_SIZE, ((budget - 2) - 1) * CELL_SIZE)
    } else {
        let solved_width = ideal_width.trunc() as u64;
        let solved_height = ideal_height.trunc() as u64;
        let scale = ((solved_width * CELL_SIZE) as f64 / width as f64)
            .min((solved_height * CELL_SIZE) as f64 / height as f64);
        (
            ((height as f64 * scale / PATCH_SIZE as f64).trunc() as u64) * PATCH_SIZE,
            ((width as f64 * scale / PATCH_SIZE as f64).trunc() as u64) * PATCH_SIZE,
        )
    };
    let grid_height = grid_cells(best_height);
    let grid_width = grid_cells(best_width);
    GridResize {
        grid_height,
        grid_width,
        best_height,
        best_width,
        num_tokens: grid_tokens(grid_height, grid_width),
    }
}

fn resize_once(width: u64, height: u64) -> GridResize {
    let (mut scaled_width, mut scaled_height) = (width, height);
    let pixels = width.saturating_mul(height);
    if pixels < MIN_PIXELS && pixels > 0 {
        let scale = (MIN_PIXELS as f64 / pixels as f64).sqrt();
        scaled_width = (scaled_width as f64 * scale).trunc() as u64;
        scaled_height = (scaled_height as f64 * scale).trunc() as u64;
    }
    let padded_width = ceil_div(scaled_width, PATCH_SIZE) * PATCH_SIZE;
    let padded_height = ceil_div(scaled_height, PATCH_SIZE) * PATCH_SIZE;
    let grid_height = grid_cells(padded_height);
    let grid_width = grid_cells(padded_width);
    let direct = GridResize {
        grid_height,
        grid_width,
        best_height: padded_height,
        best_width: padded_width,
        num_tokens: grid_tokens(grid_height, grid_width),
    };
    if direct.num_tokens <= MAX_IMAGE_TOKENS {
        direct
    } else {
        solve_resize_ratio(scaled_height, scaled_width, MAX_IMAGE_TOKENS)
    }
}

pub fn deep_seek_request_image_dimensions(width: u64, height: u64) -> ProjectedDimensions {
    let padded_width = ceil_div(width, PATCH_SIZE) * PATCH_SIZE;
    let padded_height = ceil_div(height, PATCH_SIZE) * PATCH_SIZE;
    if grid_tokens(grid_cells(padded_height), grid_cells(padded_width)) <= MAX_IMAGE_TOKENS {
        return ProjectedDimensions { width, height };
    }
    let solved = solve_resize_ratio(height, width, MAX_IMAGE_TOKENS);
    long_edge_dimensions(
        width,
        height,
        if width >= height {
            solved.best_width
        } else {
            solved.best_height
        },
    )
}

pub fn deep_seek_image_tokens(width: u64, height: u64) -> u64 {
    let mut result = resize_once(width, height);
    for _ in 1..10 {
        let next = resize_once(result.best_width, result.best_height);
        if next == result {
            return result.num_tokens;
        }
        result = next;
    }
    panic!("deepseek image tokens: resize did not converge for {width}x{height}")
}
