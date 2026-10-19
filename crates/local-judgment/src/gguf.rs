use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, Result};

#[derive(Debug)]
pub(crate) struct Info {
    pub architecture: String,
    pub template: Option<String>,
    pub tensors: u64,
    pub file_bytes: u64,
}

pub(crate) fn builtin_template(template: &str) -> Option<&'static str> {
    const BUILTINS: &[&str] = &[
        "chatml",
        "llama2",
        "llama2-sys",
        "llama2-sys-bos",
        "llama2-sys-strip",
        "mistral-v1",
        "mistral-v3",
        "mistral-v3-tekken",
        "mistral-v7",
        "mistral-v7-tekken",
        "phi3",
        "phi4",
        "falcon3",
        "zephyr",
        "monarch",
        "gemma",
        "orion",
        "openchat",
        "vicuna",
        "vicuna-orca",
        "deepseek",
        "deepseek2",
        "deepseek3",
        "deepseek-ocr",
        "command-r",
        "llama3",
        "chatglm3",
        "chatglm4",
        "glmedge",
        "minicpm",
        "exaone3",
        "exaone4",
        "exaone-moe",
        "rwkv-world",
        "granite",
        "granite-4.0",
        "granite-4.1",
        "gigachat",
        "megrez",
        "yandex",
        "bailing",
        "bailing-think",
        "bailing2",
        "llama4",
        "smolvlm",
        "hunyuan-moe",
        "gpt-oss",
        "hunyuan-dense",
        "hunyuan-vl",
        "kimi-k2",
        "seed_oss",
        "grok-2",
        "pangu-embedded",
        "solar-open",
    ];
    if let Some(name) = BUILTINS.iter().copied().find(|name| *name == template) {
        return Some(name);
    }
    let contains = |text: &str| template.contains(text);
    Some(if contains("<|im_start|>") {
        if contains("<|im_sep|>") {
            "phi4"
        } else if contains("<end_of_utterance>") {
            "smolvlm"
        } else {
            "chatml"
        }
    } else if template.starts_with("mistral") || contains("[INST]") {
        if contains("[SYSTEM_PROMPT]") {
            "mistral-v7"
        } else if contains("' [INST] ' + system_message") || contains("[AVAILABLE_TOOLS]") {
            if contains(" [INST]") {
                "mistral-v1"
            } else if contains("\"[INST]\"") {
                "mistral-v3-tekken"
            } else {
                "mistral-v3"
            }
        } else if contains("content.strip()") {
            "llama2-sys-strip"
        } else if contains("bos_token + '[INST]") {
            "llama2-sys-bos"
        } else if contains("<<SYS>>") {
            "llama2-sys"
        } else {
            "llama2"
        }
    } else if contains("<|assistant|>") && contains("<|end|>") {
        "phi3"
    } else if contains("[gMASK]<sop>") {
        "chatglm4"
    } else if contains("<|assistant|>") && contains("<|user|>") {
        if contains("<|tool_declare|>") {
            "exaone-moe"
        } else if contains("</s>") {
            "falcon3"
        } else {
            "glmedge"
        }
    } else if contains("<|{{ item['role'] }}|>") && contains("<|begin_of_image|>") {
        "glmedge"
    } else if contains("<|user|>") && contains("<|endoftext|>") {
        "zephyr"
    } else if contains("bos_token + message['role']") {
        "monarch"
    } else if contains("<start_of_turn>") {
        "gemma"
    } else if contains("'\\n\\nAssistant: ' + eos_token") {
        "orion"
    } else if contains("GPT4 Correct ") {
        "openchat"
    } else if contains("USER: ") && contains("ASSISTANT: ") {
        if contains("SYSTEM: ") {
            "vicuna-orca"
        } else {
            "vicuna"
        }
    } else if contains("### Instruction:") && contains("<|EOT|>") {
        "deepseek"
    } else if contains("<|START_OF_TURN_TOKEN|>") && contains("<|USER_TOKEN|>") {
        "command-r"
    } else if contains("<|start_header_id|>") && contains("<|end_header_id|>") {
        "llama3"
    } else if contains("[gMASK]sop") {
        "chatglm3"
    } else if contains("<用户>") {
        "minicpm"
    } else if contains("'Assistant: ' + message['content'] + eos_token") {
        "deepseek2"
    } else if contains("<｜Assistant｜>")
        && contains("<｜User｜>")
        && contains("<｜end▁of▁sentence｜>")
    {
        "deepseek3"
    } else if contains("[|system|]") && contains("[|assistant|]") && contains("[|endofturn|]") {
        if contains("[|tool|]") {
            "exaone4"
        } else {
            "exaone3"
        }
    } else if contains("rwkv-world")
        || contains("{{- 'User: ' + message['content']|trim + '\\n\\n' -}}")
    {
        "rwkv-world"
    } else if contains("<|start_of_role|>") {
        if contains("<tool_call>") || contains("<tools>") {
            if contains("g4_default_system_message") {
                "granite-4.0"
            } else {
                "granite-4.1"
            }
        } else {
            "granite"
        }
    } else if contains(
        "message['role'] + additional_special_tokens[0] + message['content'] + additional_special_tokens[1]",
    ) {
        "gigachat"
    } else if contains("<|role_start|>") {
        "megrez"
    } else if contains(" Ассистент:") {
        "yandex"
    } else if contains("<role>ASSISTANT</role>") && contains("'HUMAN'") {
        "bailing"
    } else if contains("<role>ASSISTANT</role>") && contains("\"HUMAN\"") && contains("<think>") {
        "bailing-think"
    } else if contains("<role>ASSISTANT</role>")
        && contains("<role>HUMAN</role>")
        && contains("<|role_end|>")
    {
        "bailing2"
    } else if contains("<|header_start|>") && contains("<|header_end|>") {
        "llama4"
    } else if contains("<|endofuserprompt|>") {
        "dots1"
    } else if contains("<|extra_0|>") && contains("<|extra_4|>") {
        "hunyuan-moe"
    } else if contains("<|start|>") && contains("<|channel|>") {
        "gpt-oss"
    } else if contains("<｜hy_Assistant｜>") && contains("<｜hy_begin▁of▁sentence｜>") {
        "hunyuan-vl"
    } else if contains("<｜hy_Assistant｜>") && contains("<｜hy_place▁holder▁no▁3｜>") {
        "hunyuan-dense"
    } else if contains("<|im_assistant|>assistant<|im_middle|>") {
        "kimi-k2"
    } else if contains("<seed:bos>") {
        "seed_oss"
    } else if contains("'Assistant: '  + message['content'] + '<|separator|>") {
        "grok-2"
    } else if contains("[unused9]系统：[unused10]") {
        "pangu-embedded"
    } else if contains("<|begin|>") && contains("<|end|>") && contains("<|content|>") {
        "solar-open"
    } else {
        return None;
    })
}

pub(crate) fn inspect(path: &Path, cancel: &AtomicBool) -> Result<Info> {
    if cancel.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| Error::WeightsFormat)?;
    if !metadata.is_file() || !(24..=4 * 1024 * 1024 * 1024).contains(&metadata.len()) {
        return Err(Error::WeightsFormat);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| Error::WeightsFormat)?;
    if file.metadata().map_err(|_| Error::WeightsFormat)?.len() != metadata.len() {
        return Err(Error::SourceChanged);
    }
    let mut reader = Reader {
        file,
        position: 0,
        limit: metadata.len().min(32 * 1024 * 1024),
        cancel,
    };
    if reader.bytes::<4>()? != *b"GGUF" || reader.u32()? != 3 {
        return Err(Error::WeightsFormat);
    }
    let tensors = reader.u64()?;
    let fields = reader.u64()?;
    if !(1..=65_536).contains(&tensors) || !(1..=8_192).contains(&fields) {
        return Err(Error::WeightsFormat);
    }
    let mut architecture = None;
    let mut template = None;
    let mut alignment = 32_u64;
    let mut names = BTreeSet::new();
    for _ in 0..fields {
        let name = reader.text(512)?;
        if !names.insert(name.clone()) {
            return Err(Error::WeightsFormat);
        }
        let kind = reader.u32()?;
        match name.as_str() {
            "general.architecture" => {
                if kind != 8 {
                    return Err(Error::WeightsFormat);
                }
                let value = reader.text(160)?;
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return Err(Error::WeightsFormat);
                }
                architecture = Some(value);
            }
            "tokenizer.chat_template" => {
                if kind != 8 {
                    return Err(Error::WeightsFormat);
                }
                template = Some(reader.text(256 * 1024)?);
            }
            "general.alignment" => {
                alignment = reader.count(kind)?;
                if alignment == 0 || alignment > 4096 || !alignment.is_power_of_two() {
                    return Err(Error::WeightsFormat);
                }
            }
            "split.count" => {
                if reader.count(kind)? != 1 {
                    return Err(Error::ModelFormatUnsupported);
                }
            }
            "split.no" => {
                if reader.count(kind)? != 0 {
                    return Err(Error::ModelFormatUnsupported);
                }
            }
            "split.tensors.count" => {
                if reader.count(kind)? != tensors {
                    return Err(Error::ModelFormatUnsupported);
                }
            }
            _ => reader.skip_value(kind)?,
        }
    }
    let mut offsets = Vec::new();
    names.clear();
    for _ in 0..tensors {
        let name = reader.text(512)?;
        if name.is_empty() || !names.insert(name) {
            return Err(Error::WeightsFormat);
        }
        let dimensions = reader.u32()?;
        if !(1..=4).contains(&dimensions) {
            return Err(Error::WeightsFormat);
        }
        let mut elements = 1_u64;
        for _ in 0..dimensions {
            let dimension = reader.u64()?;
            if dimension == 0 {
                return Err(Error::WeightsFormat);
            }
            elements = elements
                .checked_mul(dimension)
                .ok_or(Error::WeightsFormat)?;
        }
        let _tensor_type = reader.u32()?;
        let offset = reader.u64()?;
        if offset % alignment != 0 {
            return Err(Error::WeightsFormat);
        }
        offsets.push((offset, elements.div_ceil(8)));
    }
    let data = reader
        .position
        .div_ceil(alignment)
        .checked_mul(alignment)
        .ok_or(Error::WeightsFormat)?;
    let available = metadata
        .len()
        .checked_sub(data)
        .ok_or(Error::WeightsFormat)?;
    if offsets.into_iter().any(|(offset, minimum)| {
        offset
            .checked_add(minimum)
            .is_none_or(|end| end > available)
    }) {
        return Err(Error::WeightsFormat);
    }
    Ok(Info {
        architecture: architecture.ok_or(Error::WeightsFormat)?,
        template,
        tensors,
        file_bytes: metadata.len(),
    })
}

struct Reader<'a> {
    file: File,
    position: u64,
    limit: u64,
    cancel: &'a AtomicBool,
}

impl Reader<'_> {
    fn advance(&mut self, bytes: u64) -> Result<()> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        self.position = self
            .position
            .checked_add(bytes)
            .filter(|end| *end <= self.limit)
            .ok_or(Error::WeightsFormat)?;
        Ok(())
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.advance(N as u64)?;
        let mut bytes = [0; N];
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| Error::WeightsFormat)?;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }

    fn skip(&mut self, bytes: u64) -> Result<()> {
        self.advance(bytes)?;
        self.file
            .seek(SeekFrom::Start(self.position))
            .map_err(|_| Error::WeightsFormat)?;
        Ok(())
    }

    fn text(&mut self, maximum: u64) -> Result<String> {
        let length = self.u64()?;
        if length > maximum {
            return Err(Error::WeightsFormat);
        }
        self.advance(length)?;
        let mut bytes = vec![0; length as usize];
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| Error::WeightsFormat)?;
        String::from_utf8(bytes).map_err(|_| Error::WeightsFormat)
    }

    fn count(&mut self, kind: u32) -> Result<u64> {
        match kind {
            0 => Ok(u64::from(self.bytes::<1>()?[0])),
            2 => Ok(u64::from(u16::from_le_bytes(self.bytes()?))),
            4 => Ok(u64::from(self.u32()?)),
            10 => self.u64(),
            _ => Err(Error::WeightsFormat),
        }
    }

    fn skip_value(&mut self, kind: u32) -> Result<()> {
        match kind {
            0 | 1 | 7 => self.skip(1),
            2 | 3 => self.skip(2),
            4..=6 => self.skip(4),
            10..=12 => self.skip(8),
            8 => {
                let length = self.u64()?;
                self.skip(length)
            }
            9 => {
                let kind = self.u32()?;
                let length = self.u64()?;
                if length > 1_000_000 || kind == 9 {
                    return Err(Error::WeightsFormat);
                }
                let width = match kind {
                    0 | 1 | 7 => 1,
                    2 | 3 => 2,
                    4..=6 => 4,
                    10..=12 => 8,
                    8 => 0,
                    _ => return Err(Error::WeightsFormat),
                };
                if width != 0 {
                    return self.skip(length.checked_mul(width).ok_or(Error::WeightsFormat)?);
                }
                for _ in 0..length {
                    self.skip_value(kind)?;
                }
                Ok(())
            }
            _ => Err(Error::WeightsFormat),
        }
    }
}
