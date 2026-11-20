use hoh_attachment::{AttachmentId, ImageAttachmentRef, ImageMediaType, ProjectedDimensions};
use hoh_llm::{ContentBlock, ImageBlock};
use hoh_office_kit::{OfficeConversionFormat, OfficeRecalculationFormat};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeRenderedImage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    pub attachment_id: String,
    pub media_type: ImageMediaType,
    pub bytes: u64,
    pub width: u64,
    pub height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_dimensions: Option<ProjectedDimensions>,
}

impl OfficeRenderedImage {
    fn attachment(&self) -> ImageAttachmentRef {
        ImageAttachmentRef {
            attachment_id: AttachmentId::from(self.attachment_id.clone()),
            media_type: self.media_type,
            bytes: self.bytes,
            width: self.width,
            height: self.height,
            name: None,
            original_dimensions: self.original_dimensions,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeRenderValue {
    pub operation: String,
    pub path: String,
    pub page_count: u32,
    pub dpi: u32,
    pub images: Vec<OfficeRenderedImage>,
    pub missing_fonts: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeFileFormat {
    Pdf,
    Docx,
    Odt,
    Txt,
    Xlsx,
    Ods,
    Csv,
    Pptx,
    Odp,
}

impl std::fmt::Display for OfficeFileFormat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = serde_json::to_value(self).map_err(|_| std::fmt::Error)?;
        formatter.write_str(value.as_str().ok_or(std::fmt::Error)?)
    }
}

impl From<OfficeConversionFormat> for OfficeFileFormat {
    fn from(format: OfficeConversionFormat) -> Self {
        match format {
            OfficeConversionFormat::Pdf => Self::Pdf,
            OfficeConversionFormat::Docx => Self::Docx,
            OfficeConversionFormat::Odt => Self::Odt,
            OfficeConversionFormat::Txt => Self::Txt,
            OfficeConversionFormat::Xlsx => Self::Xlsx,
            OfficeConversionFormat::Ods => Self::Ods,
            OfficeConversionFormat::Csv => Self::Csv,
            OfficeConversionFormat::Pptx => Self::Pptx,
            OfficeConversionFormat::Odp => Self::Odp,
        }
    }
}

impl From<OfficeRecalculationFormat> for OfficeFileFormat {
    fn from(format: OfficeRecalculationFormat) -> Self {
        match format {
            OfficeRecalculationFormat::Xlsx => Self::Xlsx,
            OfficeRecalculationFormat::Ods => Self::Ods,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeFileValue {
    pub operation: String,
    pub path: String,
    pub file: String,
    pub format: OfficeFileFormat,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    pub missing_fonts: Vec<String>,
}

fn fonts_line(fonts: &[String]) -> String {
    if fonts.is_empty() {
        String::new()
    } else {
        format!(
            "\nFonts not installed and substituted: {}.",
            fonts.join(", ")
        )
    }
}

fn location(image: &OfficeRenderedImage) -> String {
    if let Some(page) = image.page {
        return format!("Page {page}");
    }
    let Some(sheet) = &image.sheet else {
        return "Image".into();
    };
    image.range.as_ref().map_or_else(
        || format!("Sheet \"{sheet}\""),
        |range| format!("Sheet \"{sheet}\" {range}"),
    )
}

pub fn render_images_content(value: &OfficeRenderValue) -> Vec<ContentBlock> {
    let count = value.images.len();
    let plural = if count == 1 { "" } else { "s" };
    let mut blocks = vec![ContentBlock::text(format!(
        "Rendered {count} image{plural} of {} ({} in the document) at {} DPI.{}",
        value.path,
        value.page_count,
        value.dpi,
        fonts_line(&value.missing_fonts)
    ))];
    for image in &value.images {
        blocks.push(ContentBlock::text(format!(
            "{}: {}x{} px",
            location(image),
            image.width,
            image.height
        )));
        blocks.push(
            ImageBlock {
                attachment: image.attachment(),
                offloaded: None,
            }
            .into(),
        );
    }
    blocks
}

pub fn render_file_content(value: &OfficeFileValue) -> Vec<ContentBlock> {
    let verb = if value.operation == "convert" {
        "Converted"
    } else {
        "Recalculated"
    };
    let pages = value.page_count.map_or_else(String::new, |count| {
        format!(", {count} page{}", if count == 1 { "" } else { "s" })
    });
    vec![ContentBlock::text(format!(
        "{verb} {} to {} ({} bytes{pages}): {}\n\
         Copy this temporary file to its destination with the shell now; it may be deleted after a retention period or to free space, and is deleted when this conversation ends.{}",
        value.path,
        value.format,
        value.bytes,
        value.file,
        fonts_line(&value.missing_fonts)
    ))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_one_page_and_its_image_reference() {
        let blocks = render_images_content(&OfficeRenderValue {
            operation: "render".into(),
            path: "/w/deck.pptx".into(),
            page_count: 3,
            dpi: 96,
            images: vec![OfficeRenderedImage {
                page: Some(2),
                sheet: None,
                range: None,
                attachment_id: "a".into(),
                media_type: ImageMediaType::Png,
                bytes: 8,
                width: 10,
                height: 20,
                original_dimensions: None,
            }],
            missing_fonts: vec![],
        });
        assert_eq!(
            blocks[0].text_value(),
            Some("Rendered 1 image of /w/deck.pptx (3 in the document) at 96 DPI.")
        );
        assert_eq!(blocks[1].text_value(), Some("Page 2: 10x20 px"));
        assert_eq!(blocks[2].image().unwrap().attachment.attachment_id, "a");
    }

    #[test]
    fn renders_sheet_locations_and_missing_fonts() {
        let value = OfficeRenderValue {
            operation: "render".into(),
            path: "/w/book.xlsx".into(),
            page_count: 2,
            dpi: 144,
            images: vec![
                OfficeRenderedImage {
                    page: None,
                    sheet: Some("Data".into()),
                    range: Some("A1:B2".into()),
                    attachment_id: "a".into(),
                    media_type: ImageMediaType::Webp,
                    bytes: 1,
                    width: 2,
                    height: 3,
                    original_dimensions: None,
                },
                OfficeRenderedImage {
                    page: None,
                    sheet: Some("Data".into()),
                    range: None,
                    attachment_id: "b".into(),
                    media_type: ImageMediaType::Jpeg,
                    bytes: 1,
                    width: 4,
                    height: 5,
                    original_dimensions: None,
                },
            ],
            missing_fonts: vec!["Aptos".into()],
        };
        let blocks = render_images_content(&value);
        assert!(blocks[0].text_value().unwrap().contains("2 images"));
        assert!(
            blocks[0]
                .text_value()
                .unwrap()
                .contains("Fonts not installed and substituted: Aptos.")
        );
        assert_eq!(blocks[1].text_value(), Some("Sheet \"Data\" A1:B2: 2x3 px"));
        assert_eq!(blocks[3].text_value(), Some("Sheet \"Data\": 4x5 px"));
    }

    #[test]
    fn keeps_normalized_image_dimensions() {
        let image = OfficeRenderedImage {
            page: None,
            sheet: None,
            range: None,
            attachment_id: "a".into(),
            media_type: ImageMediaType::Png,
            bytes: 1,
            width: 1,
            height: 1,
            original_dimensions: Some(ProjectedDimensions {
                width: 4,
                height: 5,
            }),
        };
        let blocks = render_images_content(&OfficeRenderValue {
            operation: "render".into(),
            path: "/a.pdf".into(),
            page_count: 1,
            dpi: 72,
            images: vec![image],
            missing_fonts: vec![],
        });
        assert_eq!(blocks[1].text_value(), Some("Image: 1x1 px"));
        assert_eq!(
            blocks[2].image().unwrap().attachment.original_dimensions,
            Some(ProjectedDimensions {
                width: 4,
                height: 5
            })
        );
    }

    fn file(operation: &str, pages: Option<u32>, fonts: Vec<String>) -> OfficeFileValue {
        OfficeFileValue {
            operation: operation.into(),
            path: "/w/book.xlsx".into(),
            file: "/h/book.ods".into(),
            format: OfficeFileFormat::Ods,
            bytes: 9,
            page_count: pages,
            missing_fonts: fonts,
        }
    }

    #[test]
    fn renders_conversion_copy_instruction_and_page_pluralization() {
        let text = render_file_content(&file("convert", Some(2), vec![]))[0]
            .text_value()
            .unwrap()
            .to_owned();
        assert!(
            text.starts_with("Converted /w/book.xlsx to ods (9 bytes, 2 pages): /h/book.ods\n")
        );
        assert!(text.contains("Copy this temporary file to its destination with the shell now"));
    }

    #[test]
    fn renders_recalculation_and_fonts() {
        let text = render_file_content(&file("recalculate", Some(1), vec!["Aptos".into()]))[0]
            .text_value()
            .unwrap()
            .to_owned();
        assert!(text.starts_with("Recalculated /w/book.xlsx to ods (9 bytes, 1 page)"));
        assert!(text.ends_with("Fonts not installed and substituted: Aptos."));
    }
}
