//! What a Chat's agent gets of a message's attachments (#428, #430): text pasted as a chip, rich
//! text as Markdown, where it was pasted; a Word file or a PDF as its text. A session reads a
//! file, or a paste too long to send whole, from its copy in the session's folder.

use std::sync::{Arc, Mutex};

use super::chat_thread_tests::{answered, new_chat};
use super::{Flow, Options, Reply, Script};
use crate::documents::tests::{docx, pdf};

/// Rich text pasted as a chip and a `.docx` attached reach a Chat as text, the paste where it
/// was pasted, the Word file's paragraphs and footnotes under its name.
#[tokio::test]
async fn a_chat_gets_a_pasted_markdown_chip_and_a_word_files_text() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = inputs.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.chat().is_some() {
                seen.lock().unwrap().push(turn.input.clone());
            }
            Reply::text("Here is the essay.")
        })
    });
    let flow = Flow::start("chat-attachments", Options::default(), script).await;
    let chat = new_chat(&flow).await;
    let mut pasted = flow
        .core
        .add_attachment(
            "Pasted text.md".into(),
            "text/markdown".into(),
            b"## Requirements\n\n- ten pages".to_vec(),
            true,
        )
        .await
        .unwrap();
    pasted.inline = Some(1);
    let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
    let word = flow
        .core
        .add_attachment(
            "Lucrul individual.docx".into(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into(),
            docx(&[
                (
                    "word/document.xml",
                    &format!(
                        r#"<w:document {w}><w:body><w:p><w:r><w:t>Politici anticorupție</w:t></w:r></w:p></w:body></w:document>"#
                    ),
                ),
                (
                    "word/footnotes.xml",
                    &format!(
                        r#"<w:footnotes {w}><w:footnote w:id="1"><w:p><w:r><w:t>Legea nr. 82</w:t></w:r></w:p></w:footnote></w:footnotes>"#
                    ),
                ),
            ]),
            false,
        )
        .await
        .unwrap();
    flow.manager
        .send_message(
            chat.clone(),
            "Fa-mi te rog un referat: [Pasted text #1] . La urma nu uita sa adaugi note de subsol"
                .into(),
            vec![word, pasted],
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    answered(&flow, &chat).await;
    let input = inputs.lock().unwrap().join("\n");
    assert!(
        input.contains(
            "Fa-mi te rog un referat: [Pasted text #1, pasted here by the user:]\n## Requirements\n\n- ten pages\n[end of pasted text #1] . La urma"
        ),
        "{input}"
    );
    assert!(
        input.contains(
            "[attached file \"Lucrul individual.docx\" (Word document), its text:]\nPolitici anticorupție\n\nFootnotes:\nLegea nr. 82\n[end of \"Lucrul individual.docx\"]"
        ),
        "{input}"
    );
    assert!(!input.contains("cannot be read"), "{input}");
    flow.stop().await;
}

/// An attached PDF reaches a Chat as its text, under its name; one that can't be read is
/// named as such (#430).
#[tokio::test]
async fn a_chat_gets_a_pdfs_text() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = inputs.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.chat().is_some() {
                seen.lock().unwrap().push(turn.input.clone());
            }
            Reply::text("PELICAN-42")
        })
    });
    let flow = Flow::start("chat-pdf", Options::default(), script).await;
    let chat = new_chat(&flow).await;
    let read = flow
        .core
        .add_attachment(
            "Referat.pdf".into(),
            "application/pdf".into(),
            pdf(&[&["Secret word: PELICAN-42"], &["Bibliografie: legis.md"]]),
            false,
        )
        .await
        .unwrap();
    let broken = flow
        .core
        .add_attachment(
            "Scan.pdf".into(),
            "application/pdf".into(),
            b"%PDF-1.4 not really".to_vec(),
            false,
        )
        .await
        .unwrap();
    flow.manager
        .send_message(
            chat.clone(),
            "What is the secret word?".into(),
            vec![read, broken],
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    answered(&flow, &chat).await;
    let input = inputs.lock().unwrap().join("\n");
    let start = "[attached file \"Referat.pdf\" (PDF), its text:]\nSecret word: PELICAN-42\n";
    assert!(input.contains(start), "{input}");
    assert!(
        input.contains("PELICAN-42\n\nBibliografie: legis.md\n[end of \"Referat.pdf\"]"),
        "{input}"
    );
    assert!(
        input.contains(
            "[attached file \"Scan.pdf\" (application/pdf) is not text and cannot be read in a Chat]"
        ),
        "{input}"
    );
    flow.stop().await;
}

/// A session's thread is told where the copies are: an attached file (it has no tool to read
/// one by id), and the whole of a paste too long to send.
#[tokio::test]
async fn a_session_reads_an_attached_file_and_a_long_paste_from_their_copies() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = inputs.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            seen.lock().unwrap().push(turn.input.clone());
            Reply::text("Read them.")
        })
    });
    let flow = Flow::start("session-attachments", Options::default(), script).await;
    let notes = flow
        .core
        .add_attachment(
            "notes.csv".into(),
            "text/csv".into(),
            b"name,size\nalpha,3\n".to_vec(),
            false,
        )
        .await
        .unwrap();
    let long = format!("start {} end", "x".repeat(300_000));
    let mut pasted = flow
        .core
        .add_attachment(
            "Pasted text.txt".into(),
            "text/plain".into(),
            long.clone().into_bytes(),
            true,
        )
        .await
        .unwrap();
    pasted.inline = Some(1);
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "Sum the sizes: [Pasted text #1]".into(),
            vec![notes, pasted],
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    flow.settled_in(&flow.conversation).await;
    let input = inputs.lock().unwrap().join("\n");
    let saved_as = |after: &str| -> String {
        let at = input
            .find(after)
            .unwrap_or_else(|| panic!("{after:?} in {input}"))
            + after.len();
        input[at..]
            .split(['\n', ']'])
            .next()
            .unwrap()
            .trim_end_matches(';')
            .to_owned()
    };
    let file = saved_as("\"notes.csv\" (text/csv, 18 bytes), saved as ");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "name,size\nalpha,3\n"
    );
    let whole = saved_as("; all of it is saved as ");
    assert_eq!(std::fs::read_to_string(&whole).unwrap(), long);
}
