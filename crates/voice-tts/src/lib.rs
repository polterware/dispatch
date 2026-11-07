//! Text-to-speech: Kokoro-82M through ONNX Runtime, the macOS `say` fallback,
//! and shared preparation for coding-agent replies.

use std::path::{Path, PathBuf};

use voice_core::protocol::{Language, TextFormat};

pub mod text {
    use super::*;

    pub const MAX_SPOKEN_CHARS: usize = 8000;
    const OMIT_PT: &str = "trecho de código omitido";
    const OMIT_EN: &str = "code omitted";
    const TRUNC_PT: &str = "Resposta resumida; veja o restante na tela.";
    const TRUNC_EN: &str = "The rest is on screen.";

    fn portuguese(language: Language) -> bool {
        matches!(language, Language::PtBr)
    }

    pub fn speakable(input: &str, format: TextFormat, language: Language) -> String {
        let mut source = input.to_string();
        if format == TextFormat::Plain {
            source = source.split_whitespace().collect::<Vec<_>>().join(" ");
        } else {
            source = clean_markdown(&source, portuguese(language));
        }
        let source = source.split_whitespace().collect::<Vec<_>>().join(" ");
        if source.chars().count() <= MAX_SPOKEN_CHARS {
            return source;
        }
        let limit = MAX_SPOKEN_CHARS.saturating_sub(if portuguese(language) {
            TRUNC_PT.chars().count() + 1
        } else {
            TRUNC_EN.chars().count() + 1
        });
        let head = source.chars().take(limit).collect::<String>();
        let cut = head
            .rfind(['.', '!', '?', ';', ','])
            .filter(|&i| i > limit / 2)
            .unwrap_or_else(|| head.rfind(' ').unwrap_or(head.len()));
        let spoken =
            head[..cut].trim_end_matches(|c: char| c.is_whitespace() || ",;:-".contains(c));
        format!(
            "{} {}",
            spoken,
            if portuguese(language) {
                TRUNC_PT
            } else {
                TRUNC_EN
            }
        )
    }

    fn clean_markdown(input: &str, pt: bool) -> String {
        let mut out = Vec::new();
        let omitted = if pt { OMIT_PT } else { OMIT_EN };
        let mut fence = false;
        let mut html = false;
        let mut in_diff = false;
        for raw in input.lines() {
            let indented_code = raw.starts_with("    ") || raw.starts_with('\t');
            let line = raw.trim();
            if line.starts_with("```") || line.starts_with("~~~") {
                fence = !fence;
                if fence {
                    push_omission(&mut out, omitted);
                }
                continue;
            }
            if fence {
                continue;
            }
            if line.is_empty() {
                continue;
            }
            if line.starts_with("<")
                && (line.starts_with("<html")
                    || line.starts_with("<div")
                    || line.starts_with("<pre")
                    || line.starts_with("<script"))
            {
                html = true;
                push_omission(&mut out, omitted);
                continue;
            }
            if html {
                if line.contains("</html>")
                    || line.contains("</div>")
                    || line.contains("</pre>")
                    || line.contains("</script>")
                {
                    html = false;
                }
                continue;
            }
            if line.starts_with("diff --git ") {
                in_diff = true;
                push_omission(&mut out, omitted);
                continue;
            }
            if line.starts_with("\\ No newline at end of file") {
                in_diff = true;
                push_omission(&mut out, omitted);
                continue;
            }
            if line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with("@@")
                || in_diff
                    && [
                        "index ",
                        "new file mode ",
                        "deleted file mode ",
                        "similarity index ",
                        "rename from ",
                        "rename to ",
                        "Binary files ",
                    ]
                    .iter()
                    .any(|prefix| line.starts_with(prefix))
            {
                in_diff = true;
                push_omission(&mut out, omitted);
                continue;
            }
            if indented_code
                || in_diff && (raw.starts_with('+') || raw.starts_with('-') || raw.starts_with(' '))
            {
                push_omission(&mut out, omitted);
                continue;
            }
            in_diff = false;
            if line.contains('|') {
                push_omission(&mut out, omitted);
                continue;
            }
            if line.starts_with("![") {
                push_omission(&mut out, omitted);
                continue;
            }
            if line.starts_with('<') {
                push_omission(&mut out, omitted);
                continue;
            }
            let mut text = line.to_string();
            if let Some((number, rest)) = text.split_once(". ") {
                if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
                    text = rest.to_string();
                }
            }
            for prefix in [
                "# ", "## ", "### ", "#### ", "##### ", "###### ", "> ", "- ", "* ", "+ ",
            ] {
                if text.starts_with(prefix) {
                    text = text[prefix.len()..].to_string();
                    break;
                }
            }
            text = remove_markdown_links(&text);
            text = remove_urls(&text);
            text = inline_code(&text);
            text = text
                .replace("**", "")
                .replace("__", "")
                .replace(['*', '_'], " ")
                .replace('`', "");
            text = shorten_paths(&text);
            let text = text.trim();
            if !text.is_empty() {
                out.push(text.to_string());
            }
        }
        out.join(" ")
    }
    fn push_omission(out: &mut Vec<String>, phrase: &str) {
        if out.last().is_none_or(|s| s != phrase) {
            out.push(phrase.to_string());
        }
    }
    fn remove_urls(s: &str) -> String {
        s.split_whitespace()
            .filter(|part| !part.starts_with("http://") && !part.starts_with("https://"))
            .collect::<Vec<_>>()
            .join(" ")
    }
    fn remove_markdown_links(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(link_end) = rest.find("](") {
            let Some(label_start) = rest[..link_end].rfind('[') else {
                out.push_str(&rest[..link_end + 2]);
                rest = &rest[link_end + 2..];
                continue;
            };
            let Some(url_end) = rest[link_end + 2..].find(')') else {
                break;
            };
            out.push_str(&rest[..label_start]);
            out.push_str(&rest[label_start + 1..link_end]);
            rest = &rest[link_end + 2 + url_end + 1..];
        }
        out.push_str(rest);
        out
    }
    fn inline_code(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(start) = rest.find('`') {
            out.push_str(&rest[..start]);
            rest = &rest[start + 1..];
            if let Some(end) = rest.find('`') {
                let code = &rest[..end];
                if code.chars().count() <= 24 || code.contains('/') {
                    out.push_str(code);
                }
                rest = &rest[end + 1..];
            } else {
                out.push_str(rest);
                return out;
            }
        }
        out.push_str(rest);
        out
    }
    fn shorten_paths(s: &str) -> String {
        s.split_whitespace()
            .map(|w| {
                let clean = w.trim_matches(|c: char| ",;()[]{}".contains(c));
                let (path, punctuation) = clean
                    .strip_suffix('.')
                    .map_or((clean, ""), |path| (path, "."));
                if path.contains('/') && !path.starts_with("http") {
                    format!("{}{}", path.rsplit('/').next().unwrap_or(path), punctuation)
                } else {
                    w.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn sentences(text: &str) -> Vec<String> {
        let mut pieces = Vec::new();
        let mut start = 0;
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        for (i, &(byte, ch)) in chars.iter().enumerate() {
            if matches!(ch, '.' | '!' | '?')
                && (i + 1 == chars.len() || chars[i + 1].1.is_whitespace())
            {
                let end = byte + ch.len_utf8();
                let s = text[start..end].trim();
                if !s.is_empty() {
                    pieces.extend(split_long(s));
                }
                start = end;
            }
        }
        let tail = text[start..].trim();
        if !tail.is_empty() {
            pieces.extend(split_long(tail));
        }
        pieces
    }
    fn split_long(s: &str) -> Vec<String> {
        let mut rest = s.trim();
        let mut out = Vec::new();
        while rest.chars().count() > 300 {
            let upto = rest
                .char_indices()
                .take_while(|(i, _)| *i <= 300)
                .collect::<Vec<_>>();
            let limit = upto
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(rest.len());
            let early_comma = if out.is_empty() {
                rest.char_indices()
                    .take_while(|(i, _)| *i <= 160)
                    .find(|(i, c)| *i >= 60 && *c == ',')
                    .map(|(i, c)| i + c.len_utf8())
            } else {
                None
            };
            let split = if let Some(comma) = early_comma {
                comma
            } else {
                rest[..limit].rfind([',', ';', ' ']).unwrap_or(limit)
            };
            if split == 0 {
                break;
            }
            out.push(rest[..split].trim().to_string());
            rest = rest[split..].trim();
        }
        if !rest.is_empty() {
            out.push(rest.to_string());
        }
        out
    }
    pub fn detect_language(text: &str) -> Language {
        let lower = text.to_lowercase();
        let pt_words = [
            " de ",
            " que ",
            " não ",
            " para ",
            " com ",
            " uma ",
            " por ",
            " está ",
            " foi ",
            " arquivo ",
            " testes ",
        ];
        let en_words = [
            " the ", " and ", " is ", " to ", " of ", " for ", " with ", " this ", " that ",
            " file ", " tests ",
        ];
        let pt = pt_words
            .iter()
            .map(|w| lower.matches(w).count())
            .sum::<usize>()
            * 2
            + lower.chars().filter(|c| "ãõáéíóúâêôç".contains(*c)).count();
        let en = en_words
            .iter()
            .map(|w| lower.matches(w).count())
            .sum::<usize>()
            * 2;
        if pt > en {
            Language::PtBr
        } else {
            Language::En
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn removes_agent_markup_pt() {
            let s = "## Alterações\nCriei `Button.tsx` em `src/components/Button.tsx:42`. Veja [o patch](https://example.com/a).\n```tsx\nconst a = 1;\n```\n\n- Os testes passaram.";
            let got = speakable(s, TextFormat::Markdown, Language::PtBr);
            assert!(got.contains("Button.tsx:42"));
            assert!(got.contains("patch"));
            assert!(!got.contains("example.com"));
            assert!(got.contains(OMIT_PT));
            assert!(got.contains("Os testes passaram"));
        }
        #[test]
        fn removes_diff_table_and_collapses_omissions() {
            let s = "Resposta pronta.\n```\nx\n```\n```\ny\n```\n| a | b |\n|---|---|\n| c | d |\n```diff\n+new\n-old\n```\nFim.";
            let got = speakable(s, TextFormat::Markdown, Language::En);
            assert_eq!(got.matches(OMIT_EN).count(), 1);
            assert!(got.starts_with("Resposta pronta."));
            assert!(got.ends_with("Fim."));
        }
        #[test]
        fn english_markup_and_urls() {
            let got = speakable("# Summary\nThe fix in `src/lib.rs` is ready. https://example.com\n\n```rust\nfn main() {}\n```", TextFormat::Markdown, Language::En);
            assert_eq!(got, "Summary The fix in lib.rs is ready. code omitted");
        }
        #[test]
        fn plain_collapses_and_truncates_in_language() {
            let input = format!("{} Fim.", "palavra ".repeat(MAX_SPOKEN_CHARS / 8 + 50));
            let got = speakable(&input, TextFormat::Plain, Language::PtBr);
            assert!(got.ends_with(TRUNC_PT));
            assert!(got.chars().count() <= MAX_SPOKEN_CHARS);
        }
        #[test]
        fn splits_long_first_sentence_and_detects_language() {
            let s = format!(
                "{} e depois termina.",
                "a frase tem várias palavras".repeat(16)
            );
            let p = sentences(&s);
            assert!(p.len() > 1);
            assert!(p[0].len() <= 300);
            assert_eq!(
                detect_language("Não encontrei o arquivo e os testes passaram."),
                Language::PtBr
            );
            assert_eq!(
                detect_language("The file is ready and all tests passed."),
                Language::En
            );
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("model file missing: {0}")]
    ModelMissing(PathBuf),
    #[error("espeak-ng not found; install it (brew install espeak-ng)")]
    PhonemizerMissing,
    #[error("model failed to load: {0}")]
    Load(String),
    #[error("synthesis failed: {0}")]
    Synthesis(String),
}
pub type Result<T> = std::result::Result<T, TtsError>;
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoiceSpec {
    pub language: Language,
    pub name: Option<String>,
    pub speed: f32,
}
pub trait TtsBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn sample_rate(&self) -> u32;
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>>;
}
#[derive(Debug, Clone)]
pub struct KokoroOptions {
    pub dir: PathBuf,
    pub model_file: String,
    pub espeak: Option<PathBuf>,
    pub threads: usize,
}

mod kokoro;
pub mod piper;
pub use piper::{PiperOptions, PiperTts};
pub use kokoro::KokoroTts;
impl KokoroTts {
    pub fn load(options: KokoroOptions) -> Result<Self> {
        kokoro::load(options)
    }
}
impl TtsBackend for KokoroTts {
    fn name(&self) -> &'static str {
        "kokoro"
    }
    fn sample_rate(&self) -> u32 {
        24_000
    }
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
        kokoro::synthesize(self, text, voice)
    }
}

mod system;
pub use system::SystemTts;
impl SystemTts {
    pub fn new() -> Self {
        system::new()
    }
    pub fn available() -> bool {
        cfg!(target_os = "macos")
    }
}
impl Default for SystemTts {
    fn default() -> Self {
        Self::new()
    }
}
impl TtsBackend for SystemTts {
    fn name(&self) -> &'static str {
        "system"
    }
    fn sample_rate(&self) -> u32 {
        22_050
    }
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
        system::synthesize(text, voice)
    }
}
pub fn kokoro_installed(dir: &Path) -> bool {
    dir.join("tokenizer.json").exists() && dir.join("voices").is_dir()
}
