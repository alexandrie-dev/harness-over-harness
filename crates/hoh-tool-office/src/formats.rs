use std::collections::HashSet;

use hoh_error::HarnessError;
use hoh_office_kit::{OfficeConversionFormat, OfficeRecalculationFormat};
use hoh_tools::ToolArgsError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeOperationName {
    Render,
    Convert,
    Recalculate,
}

impl OfficeOperationName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Render => "render",
            Self::Convert => "convert",
            Self::Recalculate => "recalculate",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeFamily {
    Text,
    Spreadsheet,
    Presentation,
    Pdf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeRenderArgs {
    pub input: String,
    #[serde(default)]
    pub pages: Option<Vec<i64>>,
    #[serde(default)]
    pub sheet: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub dpi: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeConvertArgs {
    pub input: String,
    pub format: OfficeConversionFormat,
    #[serde(default)]
    pub sheet: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeRecalculateArgs {
    pub input: String,
    pub format: OfficeRecalculationFormat,
}

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn integer_i64(value: &Value) -> Option<i64> {
    let number = value.as_number()?;
    number
        .as_i64()
        .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| {
            number.as_f64().and_then(|value| {
                (value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER)
                    .then_some(value as i64)
            })
        })
}

fn deferred_integer_i64(value: &Value) -> Option<i64> {
    let number = value.as_number()?;
    Some(integer_i64(value).unwrap_or_else(|| {
        if number.as_f64().is_some_and(f64::is_sign_negative) {
            i64::MIN
        } else {
            i64::MAX
        }
    }))
}

/// Preserve JavaScript integer semantics after the tool schema accepted the raw JSON value.
pub(crate) fn parse_render_arguments(mut raw: Value) -> Result<OfficeRenderArgs, HarnessError> {
    let object = raw
        .as_object_mut()
        .expect("the tool schema validated an argument object");
    if let Some(Value::Array(pages)) = object.get_mut("pages") {
        for page in pages {
            let Some(integer) = deferred_integer_i64(page) else {
                return Err(ToolArgsError::new(vec![
                    "pages must list distinct one-based page numbers".into(),
                ]));
            };
            *page = Value::Number(Number::from(integer));
        }
    }
    if let Some(dpi) = object.get_mut("dpi") {
        let Some(integer) = deferred_integer_i64(dpi) else {
            return Err(ToolArgsError::new(vec![
                "dpi must be an integer from 24 to 600".into(),
            ]));
        };
        *dpi = Value::Number(Number::from(integer));
    }
    // The tool schema has already validated all non-numeric field types.
    serde_json::from_value(raw).map_err(|error| ToolArgsError::new(vec![error.to_string()]))
}

pub const OUTPUT_FORMATS: &[&str] = &[
    "pdf", "docx", "odt", "txt", "xlsx", "ods", "csv", "pptx", "odp",
];
pub const RECALCULATION_FORMATS: &[&str] = &["xlsx", "ods"];

pub fn extension_of(path: &str) -> String {
    // Same as the source `extensionOf`: both separators on every platform.
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return String::new();
    };
    if stem.is_empty() {
        String::new()
    } else {
        extension.to_lowercase()
    }
}

pub fn input_family(
    operation: OfficeOperationName,
    path: &str,
) -> Result<OfficeFamily, HarnessError> {
    let family = match extension_of(path).as_str() {
        "doc" | "docx" | "odt" => Some(OfficeFamily::Text),
        "xls" | "xlsx" | "ods" => Some(OfficeFamily::Spreadsheet),
        "ppt" | "pptx" | "odp" => Some(OfficeFamily::Presentation),
        "pdf" => Some(OfficeFamily::Pdf),
        _ => None,
    };
    if operation == OfficeOperationName::Recalculate {
        return (family == Some(OfficeFamily::Spreadsheet))
            .then_some(OfficeFamily::Spreadsheet)
            .ok_or_else(|| {
                ToolArgsError::new(vec![format!(
                    "cannot recalculate \"{path}\": recalculate accepts xls, xlsx, or ods workbooks"
                )])
            });
    }
    if family.is_none()
        || (family == Some(OfficeFamily::Pdf) && operation == OfficeOperationName::Convert)
    {
        let formats = if operation == OfficeOperationName::Render {
            "doc, docx, odt, xls, xlsx, ods, ppt, pptx, odp, or pdf"
        } else {
            "doc, docx, odt, xls, xlsx, ods, ppt, pptx, or odp"
        };
        return Err(ToolArgsError::new(vec![format!(
            "cannot {} \"{path}\": {} accepts {formats} files",
            operation.as_str(),
            operation.as_str()
        )]));
    }
    Ok(family.expect("family was checked"))
}

pub fn check_render_arguments(
    args: &OfficeRenderArgs,
    family: OfficeFamily,
) -> Result<(), HarnessError> {
    if let Some(pages) = &args.pages {
        if family == OfficeFamily::Spreadsheet {
            return Err(ToolArgsError::new(vec![
                "pages does not apply to spreadsheets; select sheet and range instead".into(),
            ]));
        }
        let distinct: HashSet<_> = pages.iter().copied().collect();
        if pages.is_empty()
            || pages
                .iter()
                .any(|page| *page < 1 || u32::try_from(*page).is_err())
            || distinct.len() != pages.len()
        {
            return Err(ToolArgsError::new(vec![
                "pages must list distinct one-based page numbers".into(),
            ]));
        }
    }
    if family != OfficeFamily::Spreadsheet && args.sheet.is_some() {
        return Err(ToolArgsError::new(vec![
            "sheet applies only to spreadsheets".into(),
        ]));
    }
    if args.range.is_some() && args.sheet.is_none() {
        return Err(ToolArgsError::new(vec!["range requires sheet".into()]));
    }
    if args
        .sheet
        .as_ref()
        .is_some_and(|sheet| hoh_util_values::js_trim(sheet).is_empty())
    {
        return Err(ToolArgsError::new(vec![
            "sheet must be an exact worksheet name".into(),
        ]));
    }
    if args.dpi.is_some_and(|dpi| !(24..=600).contains(&dpi)) {
        return Err(ToolArgsError::new(vec![
            "dpi must be an integer from 24 to 600".into(),
        ]));
    }
    Ok(())
}

pub fn check_conversion_arguments(
    args: &OfficeConvertArgs,
    family: OfficeFamily,
) -> Result<(), HarnessError> {
    let accepted: &[OfficeConversionFormat] = match family {
        OfficeFamily::Text => &[
            OfficeConversionFormat::Pdf,
            OfficeConversionFormat::Docx,
            OfficeConversionFormat::Odt,
            OfficeConversionFormat::Txt,
        ],
        OfficeFamily::Spreadsheet => &[
            OfficeConversionFormat::Pdf,
            OfficeConversionFormat::Xlsx,
            OfficeConversionFormat::Ods,
            OfficeConversionFormat::Csv,
        ],
        OfficeFamily::Presentation => &[
            OfficeConversionFormat::Pdf,
            OfficeConversionFormat::Pptx,
            OfficeConversionFormat::Odp,
        ],
        OfficeFamily::Pdf => &[],
    };
    if !accepted.contains(&args.format) {
        return Err(ToolArgsError::new(vec![format!(
            "cannot convert \"{}\" to {}: this file converts to {}",
            args.input,
            args.format,
            accepted
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )]));
    }
    if args.sheet.is_some() && args.format != OfficeConversionFormat::Csv {
        return Err(ToolArgsError::new(vec![
            "sheet applies only to CSV conversion".into(),
        ]));
    }
    if args
        .sheet
        .as_ref()
        .is_some_and(|sheet| hoh_util_values::js_trim(sheet).is_empty())
    {
        return Err(ToolArgsError::new(vec![
            "sheet must be an exact worksheet name".into(),
        ]));
    }
    Ok(())
}

pub fn check_parameter_names(tool: &str, args: &Map<String, Value>) -> Result<(), HarnessError> {
    const RENDER: &[&str] = &["input", "pages", "sheet", "range", "dpi"];
    const CONVERT: &[&str] = &["input", "format", "sheet"];
    const RECALCULATE: &[&str] = &["input", "format"];
    let declared = match tool {
        "office_render" => RENDER,
        "office_convert" => CONVERT,
        "office_recalculate" => RECALCULATE,
        _ => &[],
    };
    for key in args.keys() {
        if declared.contains(&key.as_str()) {
            continue;
        }
        let owners = [
            ("office_render", RENDER),
            ("office_convert", CONVERT),
            ("office_recalculate", RECALCULATE),
        ]
        .into_iter()
        .filter_map(|(owner, names)| names.contains(&key.as_str()).then_some(owner))
        .collect::<Vec<_>>();
        let violation = if owners.is_empty() {
            format!("{tool} does not accept {key}")
        } else {
            format!("{key} applies only to {}", owners.join(" or "))
        };
        return Err(ToolArgsError::new(vec![violation]));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(error: HarnessError) -> String {
        error.to_string()
    }

    #[test]
    fn extracts_extensions_like_node_path_extname() {
        assert_eq!(extension_of("/a/B.PPTX"), "pptx");
        assert_eq!(extension_of(r"C:\\a\\book.xlsx"), "xlsx");
        assert_eq!(extension_of(r"/w/a\.docx"), "");
        assert_eq!(extension_of(r"/w/a\b.docx"), "docx");
        assert_eq!(extension_of(".pptx"), "");
        assert_eq!(extension_of("name"), "");
        assert_eq!(extension_of("A.DOCX"), "docx");
    }

    #[test]
    fn selects_every_render_family_and_pdf() {
        for (path, expected) in [
            ("a.docx", OfficeFamily::Text),
            ("a.xlsx", OfficeFamily::Spreadsheet),
            ("a.pptx", OfficeFamily::Presentation),
            ("a.pdf", OfficeFamily::Pdf),
        ] {
            assert_eq!(
                input_family(OfficeOperationName::Render, path).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn rejects_operation_specific_input_extensions() {
        assert!(
            message(input_family(OfficeOperationName::Render, "a.txt").unwrap_err())
                .contains("render accepts")
        );
        assert!(
            message(input_family(OfficeOperationName::Convert, "a.pdf").unwrap_err())
                .contains("convert accepts")
        );
        assert!(
            message(input_family(OfficeOperationName::Recalculate, "a.pptx").unwrap_err())
                .contains("recalculate accepts xls, xlsx, or ods")
        );
    }

    #[test]
    fn accepts_valid_document_page_selections() {
        let args = OfficeRenderArgs {
            input: "a.pdf".into(),
            pages: Some(vec![1, 3]),
            sheet: None,
            range: None,
            dpi: Some(600),
        };
        assert!(check_render_arguments(&args, OfficeFamily::Pdf).is_ok());
    }

    #[test]
    fn rejects_invalid_page_lists() {
        for pages in [vec![], vec![0], vec![1, 1], vec![i64::from(u32::MAX) + 1]] {
            let args = OfficeRenderArgs {
                input: "a.pdf".into(),
                pages: Some(pages),
                sheet: None,
                range: None,
                dpi: None,
            };
            assert!(
                message(check_render_arguments(&args, OfficeFamily::Pdf).unwrap_err())
                    .contains("distinct one-based")
            );
        }
    }

    #[test]
    fn rejects_misplaced_render_selections() {
        let cases = [
            (
                OfficeFamily::Spreadsheet,
                Some(vec![1]),
                None,
                None,
                None,
                "pages does not apply",
            ),
            (
                OfficeFamily::Text,
                None,
                Some("Data"),
                None,
                None,
                "sheet applies only",
            ),
            (
                OfficeFamily::Spreadsheet,
                None,
                None,
                Some("A1"),
                None,
                "range requires sheet",
            ),
            (
                OfficeFamily::Spreadsheet,
                None,
                Some(" "),
                None,
                None,
                "exact worksheet",
            ),
            (
                OfficeFamily::Text,
                None,
                None,
                None,
                Some(23),
                "integer from 24 to 600",
            ),
        ];
        for (family, pages, sheet, range, dpi, expected) in cases {
            let args = OfficeRenderArgs {
                input: "a.docx".into(),
                pages,
                sheet: sheet.map(str::to_owned),
                range: range.map(str::to_owned),
                dpi,
            };
            assert!(message(check_render_arguments(&args, family).unwrap_err()).contains(expected));
        }
        let args = OfficeRenderArgs {
            input: "a.xlsx".into(),
            pages: None,
            sheet: Some("\u{feff}".into()),
            range: None,
            dpi: None,
        };
        assert!(
            message(check_render_arguments(&args, OfficeFamily::Spreadsheet).unwrap_err())
                .contains("exact worksheet")
        );
    }

    #[test]
    fn accepts_conversion_matrix() {
        for (family, formats) in [
            (
                OfficeFamily::Text,
                vec![
                    OfficeConversionFormat::Pdf,
                    OfficeConversionFormat::Docx,
                    OfficeConversionFormat::Odt,
                    OfficeConversionFormat::Txt,
                ],
            ),
            (
                OfficeFamily::Spreadsheet,
                vec![
                    OfficeConversionFormat::Pdf,
                    OfficeConversionFormat::Xlsx,
                    OfficeConversionFormat::Ods,
                    OfficeConversionFormat::Csv,
                ],
            ),
            (
                OfficeFamily::Presentation,
                vec![
                    OfficeConversionFormat::Pdf,
                    OfficeConversionFormat::Pptx,
                    OfficeConversionFormat::Odp,
                ],
            ),
        ] {
            for format in formats {
                assert!(
                    check_conversion_arguments(
                        &OfficeConvertArgs {
                            input: "input".into(),
                            format,
                            sheet: None
                        },
                        family
                    )
                    .is_ok()
                );
            }
        }
    }

    #[test]
    fn rejects_wrong_conversion_format_and_sheet_rules() {
        let wrong = OfficeConvertArgs {
            input: "deck.pptx".into(),
            format: OfficeConversionFormat::Xlsx,
            sheet: None,
        };
        assert!(
            message(check_conversion_arguments(&wrong, OfficeFamily::Presentation).unwrap_err())
                .contains("pdf, pptx, odp")
        );
        let sheet = OfficeConvertArgs {
            input: "book.xlsx".into(),
            format: OfficeConversionFormat::Pdf,
            sheet: Some("Data".into()),
        };
        assert!(
            message(check_conversion_arguments(&sheet, OfficeFamily::Spreadsheet).unwrap_err())
                .contains("CSV")
        );
        let blank = OfficeConvertArgs {
            input: "book.xlsx".into(),
            format: OfficeConversionFormat::Csv,
            sheet: Some("\u{feff}".into()),
        };
        assert!(
            message(check_conversion_arguments(&blank, OfficeFamily::Spreadsheet).unwrap_err())
                .contains("exact worksheet")
        );
    }

    #[test]
    fn names_the_tools_that_own_an_extra_parameter() {
        let args = json!({"input":"a", "format":"pdf", "output":"x"});
        let object = args.as_object().unwrap();
        assert!(
            message(check_parameter_names("office_render", object).unwrap_err())
                .contains("format applies only to office_convert or office_recalculate")
        );
        let args = json!({"input":"a", "output":"x"});
        assert!(
            message(check_parameter_names("office_render", args.as_object().unwrap()).unwrap_err())
                .contains("office_render does not accept output")
        );
    }
}
