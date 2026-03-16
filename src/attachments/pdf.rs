//! PDF text extraction for file attachments

use crate::Error;

/// Check whether an attachment is a PDF based on its MIME type
pub fn is_pdf_attachment(mime_type: &str) -> bool {
    mime_type == "application/pdf"
}

/// Extract text content from PDF binary data
///
/// # Errors
///
/// Returns error if the PDF cannot be parsed or text extraction fails
pub fn extract_pdf_text(data: &[u8]) -> crate::Result<String> {
    pdf_extract::extract_text_from_mem(data)
        .map_err(|e| Error::Attachment(format!("PDF extraction failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_pdf_mime_type() {
        assert!(is_pdf_attachment("application/pdf"));
        assert!(!is_pdf_attachment("image/png"));
        assert!(!is_pdf_attachment("text/plain"));
        assert!(!is_pdf_attachment("application/json"));
    }

    #[test]
    fn extract_rejects_invalid_data() {
        let result = extract_pdf_text(b"not a pdf");
        assert!(result.is_err());
    }
}
