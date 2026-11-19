//! Request-image projection and synchronous token pricing.

use crate::image_tokens::{deep_seek_image_tokens, deep_seek_request_image_dimensions};
use crate::types::{DeepSeekCatalogModel, DeepSeekConnectionOptions, ImagePixelBudget};
use hoh_attachment::{
    ImageAttachmentRef, ImageRequestTarget, long_edge_dimensions, request_image_dimensions,
};
use hoh_llm::{
    ImageAttachmentAccessResolver, ImageBlock, LlmImageRequestPrice, LlmImageRequestPricing,
    offloaded_image_text, request_image_handle_text, text_only_image_text,
};
use std::sync::Arc;

pub const DEFAULT_MAX_REQUEST_FILES_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_IMAGES_PER_REQUEST: u64 = 600;
pub const DEFAULT_LOW_DETAIL_IMAGE_PIXEL_BUDGET: u64 = 512 * 512;
pub const DEFAULT_REQUEST_IMAGE_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const REQUEST_IMAGE_MAX_DIMENSION: u64 = 4096;

pub fn resolve_request_image_max_bytes(model: &DeepSeekCatalogModel) -> u64 {
    model
        .image_max_bytes
        .unwrap_or(DEFAULT_REQUEST_IMAGE_MAX_BYTES)
}

pub fn resolve_request_image_target(
    model: &DeepSeekCatalogModel,
    source: &ImageAttachmentRef,
) -> ImageRequestTarget {
    let projected = match model.image_pixel_budget {
        None => deep_seek_request_image_dimensions(source.width, source.height),
        Some(ImagePixelBudget::Low(_)) => request_image_dimensions(
            source.width,
            source.height,
            DEFAULT_LOW_DETAIL_IMAGE_PIXEL_BUDGET,
        ),
        Some(ImagePixelBudget::Pixels(pixels)) => {
            request_image_dimensions(source.width, source.height, pixels)
        }
    };
    let capped = if projected.width.max(projected.height) > REQUEST_IMAGE_MAX_DIMENSION {
        long_edge_dimensions(source.width, source.height, REQUEST_IMAGE_MAX_DIMENSION)
    } else {
        projected
    };
    ImageRequestTarget {
        width: capped.width,
        height: capped.height,
        max_bytes: resolve_request_image_max_bytes(model),
    }
}

pub struct DeepSeekImageRequestPricing {
    model: Option<DeepSeekCatalogModel>,
    resolve_access: Option<Arc<ImageAttachmentAccessResolver<'static>>>,
}

impl LlmImageRequestPricing for DeepSeekImageRequestPricing {
    fn price_images(&self, images: &[ImageBlock]) -> Vec<LlmImageRequestPrice> {
        let image_capable = self
            .model
            .as_ref()
            .and_then(|model| model.input_modalities.as_ref())
            .is_some_and(|modalities| modalities.iter().any(|item| item == "image"));
        images
            .iter()
            .map(|image| {
                if !image_capable {
                    return LlmImageRequestPrice {
                        visual_tokens: 0,
                        text: text_only_image_text(&image.attachment),
                    };
                }
                let access = self
                    .resolve_access
                    .as_ref()
                    .and_then(|resolve| resolve(&image.attachment));
                if image.offloaded == Some(true) {
                    return LlmImageRequestPrice {
                        visual_tokens: 0,
                        text: offloaded_image_text(&image.attachment, access.as_ref()),
                    };
                }
                let target = resolve_request_image_target(
                    self.model.as_ref().expect("image-capable model exists"),
                    &image.attachment,
                );
                LlmImageRequestPrice {
                    visual_tokens: deep_seek_image_tokens(target.width, target.height),
                    text: request_image_handle_text(
                        &image.attachment,
                        target.width,
                        target.height,
                        access.as_ref(),
                    ),
                }
            })
            .collect()
    }
}

pub fn deep_seek_image_request_pricing(
    connection: &DeepSeekConnectionOptions,
    model: &str,
    resolve_access: Option<Arc<ImageAttachmentAccessResolver<'static>>>,
) -> DeepSeekImageRequestPricing {
    DeepSeekImageRequestPricing {
        model: connection
            .models
            .iter()
            .find(|entry| entry.id == model)
            .cloned(),
        resolve_access,
    }
}
