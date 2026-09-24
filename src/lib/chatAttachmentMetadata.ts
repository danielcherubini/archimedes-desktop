/**
 * Checks if the text content should be preferred over HTML when pasting from a spreadsheet.
 * Spreadsheet apps often put tab-separated values in text/plain and a complex HTML table in text/html.
 */
const SPREADSHEET_CLIPBOARD_HTML_PATTERN = /Excel\.Sheet/i;

export function shouldPreferSpreadsheetClipboardText(text: string, html: string): boolean {
  if (!text) return false;
  return text.includes("\t") || SPREADSHEET_CLIPBOARD_HTML_PATTERN.test(html);
}

/**
 * Maps common file extensions to MIME types.
 */
export function inferAttachmentMimeType(filename: string): string {
  const ext = filename.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "png": return "image/png";
    case "jpg":
    case "jpeg": return "image/jpeg";
    case "gif": return "image/gif";
    case "webp": return "image/webp";
    case "txt": return "text/plain";
    case "json": return "application/json";
    case "md": return "text/markdown";
    case "pdf": return "application/pdf";
    default: return "application/octet-stream";
  }
}
