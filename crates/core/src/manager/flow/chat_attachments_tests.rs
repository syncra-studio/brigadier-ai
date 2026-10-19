//! What a Chat's agent gets of a message's attachments (#428): text pasted as a chip, rich text
//! as Markdown, where it was pasted; a Word file as its text.

use std::sync::{Arc, Mutex};

use super::chat_thread_tests::{answered, new_chat};
use super::{Flow, Options, Reply, Script};
use crate::documents::tests::docx;

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
