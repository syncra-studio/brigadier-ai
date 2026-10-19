//! Text out of documents an agent can't read as they are: a Word file's paragraphs and a PDF's
//! text, for a Chat, which gets its attachments as text in the message.

use std::io::{Cursor, Read};

use quick_xml::Reader;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;

/// A Word document's MIME type.
const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// The largest part of a Word file read out (its text, unzipped).
const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;

/// Whether an attachment is a PDF, by its type or, untyped, its name.
pub(crate) fn is_pdf(name: &str, mime: &str) -> bool {
    mime == "application/pdf"
        || (!mime.starts_with("text/")
            && name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("pdf")))
}

/// A PDF's text, its pages in order, a blank line apart. None when it can't be read (not a
/// PDF, encrypted, a scan with no text, or one the reader trips on).
pub(crate) fn pdf_text(bytes: &[u8]) -> Option<String> {
    // The reader panics on some malformed files: one bad attachment never takes the turn down.
    let pages = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(bytes))
        .ok()?
        .ok()?;
    let text = pages
        .iter()
        .map(|page| page_lines(page))
        .filter(|page| !page.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// A page's lines, trimmed at the end, with one blank line at most between blocks.
fn page_lines(page: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in page.lines() {
        let line = line.replace('\u{c}', "");
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_owned()
}

/// Whether an attachment is a Word document (`.docx`), by its type or, untyped, its name.
pub(crate) fn is_docx(name: &str, mime: &str) -> bool {
    mime == DOCX_MIME
        || (!mime.starts_with("text/")
            && name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("docx")))
}

/// A Word document's text: its paragraphs a line each, then its footnotes and endnotes under
/// their own headings. None when it isn't a Word file that can be read.
pub(crate) fn docx_text(bytes: &[u8]) -> Option<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut text = paragraphs(&part(&mut archive, "word/document.xml")?)?
        .trim_end()
        .to_owned();
    for (file, heading) in [
        ("word/footnotes.xml", "Footnotes"),
        ("word/endnotes.xml", "Endnotes"),
    ] {
        let notes = part(&mut archive, file)
            .and_then(|xml| paragraphs(&xml))
            .unwrap_or_default();
        if !notes.trim().is_empty() {
            text.push_str(&format!("\n\n{heading}:\n{}", notes.trim_end()));
        }
    }
    Some(text.trim_end().to_owned())
}

fn part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<String> {
    let file = archive.by_name(name).ok()?;
    let mut xml = String::new();
    file.take(MAX_PART_BYTES).read_to_string(&mut xml).ok()?;
    Some(xml)
}

/// The text of a WordprocessingML part: runs' text, tabs and breaks, a line per paragraph and
/// table row cells apart by tabs. Empty paragraphs (a separator footnote's) are left out.
fn paragraphs(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut line = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event().ok()? {
            Event::Start(tag) => in_text = tag.local_name().as_ref() == "t",
            Event::Empty(tag) => match tag.local_name().as_ref() {
                "tab" => line.push('\t'),
                "br" | "cr" => line.push('\n'),
                _ => {}
            },
            Event::Text(text) if in_text => line.push_str(&text.xml10_content()),
            Event::GeneralRef(entity) if in_text => {
                if let Ok(Some(ch)) = entity.resolve_char_ref() {
                    line.push(ch);
                } else if let Some(resolved) = resolve_predefined_entity(&entity.xml10_content()) {
                    line.push_str(resolved);
                }
            }
            Event::End(tag) => match tag.local_name().as_ref() {
                "t" => in_text = false,
                "p" => {
                    if !line.trim().is_empty() {
                        out.push_str(line.trim_end());
                        out.push('\n');
                    }
                    line.clear();
                }
                "tc" => {
                    if out.ends_with('\n') {
                        out.pop();
                    }
                    out.push('\t');
                }
                "tr" => {
                    if out.ends_with('\t') {
                        out.pop();
                    }
                    out.push('\n');
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    /// A Word file of `parts` (name, XML).
    pub(crate) fn docx(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, xml) in parts {
            zip.start_file(*name, options).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// A PDF of `pages`, each its lines in Helvetica, top down.
    pub(crate) fn pdf(pages: &[&[&str]]) -> Vec<u8> {
        let font = 3 + 2 * pages.len();
        let kids: Vec<String> = (0..pages.len())
            .map(|page| format!("{} 0 R", 3 + 2 * page))
            .collect();
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                pages.len()
            ),
        ];
        for (page, lines) in pages.iter().enumerate() {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {} 0 R >>",
                4 + 2 * page
            ));
            let mut content = String::from("BT /F1 12 Tf 14 TL 72 720 Td");
            for line in *lines {
                content.push_str(&format!(" ({line}) Tj T*"));
            }
            content.push_str(" ET");
            objects.push(format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ));
        }
        objects.push(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_owned(),
        );
        let mut file = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (n, object) in objects.iter().enumerate() {
            offsets.push(file.len());
            file.push_str(&format!("{} 0 obj\n{object}\nendobj\n", n + 1));
        }
        let xref = file.len();
        file.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            file.push_str(&format!("{offset:010} 00000 n \n"));
        }
        file.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        file.into_bytes()
    }

    #[test]
    fn a_pdf_reads_as_its_text_page_by_page() {
        let file = pdf(&[
            &["Lucrul individual", "Secret word: PELICAN-42"],
            &["Bibliografie: legis.md"],
        ]);
        let text = pdf_text(&file).unwrap();
        let first = text.find("Lucrul individual").expect(&text);
        let word = text.find("Secret word: PELICAN-42").expect(&text);
        let last = text.find("Bibliografie: legis.md").expect(&text);
        assert!(first < word && word < last, "{text}");
        assert!(text.contains("PELICAN-42\n\nBibliografie"), "{text:?}");
        assert!(
            !text.contains('\u{c}') && !text.contains("\n\n\n"),
            "{text:?}"
        );
        assert_eq!(pdf_text(b"not a pdf"), None);
        assert_eq!(pdf_text(b"%PDF-1.4\n1 0 obj\n<< /Type"), None);
        assert_eq!(pdf_text(&pdf(&[&[]])), None);
    }

    #[test]
    fn pdfs_are_known_by_type_or_name() {
        assert!(is_pdf("x.bin", "application/pdf"));
        assert!(is_pdf("Referat.PDF", "application/octet-stream"));
        assert!(is_pdf("a.pdf", ""));
        assert!(!is_pdf("notes.pdf.txt", "text/plain"));
        assert!(!is_pdf("a.docx", DOCX_MIME));
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    #[test]
    fn a_word_file_reads_as_its_paragraphs_then_its_footnotes() {
        let body = format!(
            r#"<w:document {W}><w:body>
            <w:p><w:r><w:t>Lucrul individual</w:t></w:r></w:p>
            <w:p/>
            <w:p><w:r><w:t xml:space="preserve">Cerinț</w:t></w:r><w:r><w:t>e &amp; note</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>
            <w:p><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t></w:r></w:p>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>y</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
            </w:body></w:document>"#
        );
        let notes = format!(
            r#"<w:footnotes {W}><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>
            <w:footnote w:id="1"><w:p><w:r><w:t>Sursa: lege.md</w:t></w:r></w:p></w:footnote></w:footnotes>"#
        );
        let file = docx(&[
            ("[Content_Types].xml", "<Types/>"),
            ("word/document.xml", &body),
            ("word/footnotes.xml", &notes),
        ]);
        assert_eq!(
            docx_text(&file).unwrap(),
            "Lucrul individual\nCerințe & note\na\tb\nc\nx\ty\n\nFootnotes:\nSursa: lege.md"
        );
        assert_eq!(docx_text(b"not a zip"), None);
        assert_eq!(docx_text(&docx(&[("other.xml", "<a/>")])), None);
    }

    #[test]
    fn word_files_are_known_by_type_or_name() {
        assert!(is_docx("x.bin", DOCX_MIME));
        assert!(is_docx(
            "Lucrul individual.DOCX",
            "application/octet-stream"
        ));
        assert!(is_docx("a.docx", ""));
        assert!(!is_docx("notes.docx.txt", "text/plain"));
        assert!(!is_docx("a.pdf", "application/pdf"));
    }
}
