use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use api::{
    AuthRoute, ProviderClient, resolve_model_alias,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use runtime::{
    PermissionMode,
    image_guard::{ImageGuardOutcome, guard_image_bytes},
    permission_enforcer::PermissionEnforcer,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    ToolContext, ToolError, ToolSpec, from_value, maybe_enforce_permission_check, to_pretty_json,
};
use crate::file_tools::sniff_image_mime;

// Bound the original saved payload. The separate image guard enforces its
// smaller provider-wire limit before attaching pixels to conversation history.
const MAX_GENERATED_IMAGE_BASE64_BYTES: usize = 40 * 1024 * 1024;
static IMAGE_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImagegenInput {
    prompt: String,
    #[serde(default)]
    size: Option<ImageSize>,
    #[serde(default)]
    quality: Option<ImageQuality>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
enum ImageSize {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "1024x1024")]
    Square,
    #[serde(rename = "1536x1024")]
    Landscape,
    #[serde(rename = "1024x1536")]
    Portrait,
}

impl ImageSize {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Square => "1024x1024",
            Self::Landscape => "1536x1024",
            Self::Portrait => "1024x1536",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ImageQuality {
    Auto,
    Low,
    Medium,
    High,
}

impl ImageQuality {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "imagegen",
        description: "Generate one image through the active OpenAI GPT model's saved OAuth subscription. The image is written under `.zo/generated-images/` and attached to the tool result so the model can inspect it. Available only for current-family GPT models with `zo login openai` credentials. This consumes the account's image-generation allowance.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "A complete visual description of the image to generate.",
                    "minLength": 1,
                    "maxLength": 32000
                },
                "size": {
                    "type": "string",
                    "enum": ["auto", "1024x1024", "1536x1024", "1024x1536"],
                    "description": "Output dimensions. Defaults to the provider's automatic choice."
                },
                "quality": {
                    "type": "string",
                    "enum": ["auto", "low", "medium", "high"],
                    "description": "Generation quality. Defaults to the provider's automatic choice."
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }),
        required_permission: PermissionMode::WorkspaceWrite,
    }]
}

/// The catalog capability a model must declare to be offered `imagegen`.
const IMAGEGEN_CAPABILITY: &str = "imagegen";

pub(crate) fn supports_model(model: &str) -> bool {
    api::model_has_capability(&resolve_model_alias(model), IMAGEGEN_CAPABILITY)
}

pub(crate) fn dispatch(
    context: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    match name {
        "imagegen" => Some(
            maybe_enforce_permission_check(enforcer, name, input)
                .and_then(|()| run_imagegen(input, context)),
        ),
        _ => None,
    }
}

fn run_imagegen(input: &Value, context: &ToolContext) -> Result<String, ToolError> {
    let input: ImagegenInput = from_value(input)?;
    let prompt = input.prompt.trim();
    if prompt.is_empty() {
        return Err(ToolError::InvalidInput(
            "imagegen prompt must not be empty".to_string(),
        ));
    }
    if prompt.chars().count() > 32_000 {
        return Err(ToolError::InvalidInput(
            "imagegen prompt must not exceed 32000 characters".to_string(),
        ));
    }

    let model = context.active_model().ok_or_else(|| {
        ToolError::Execution("imagegen requires an active GPT model".to_string())
    })?;
    if !supports_model(&model) {
        return Err(ToolError::Execution(format!(
            "imagegen requires a current-family OpenAI GPT model; active model is `{model}`"
        )));
    }

    let client = ProviderClient::from_model_with_auth_route(&model, AuthRoute::OAuth)
        .map_err(|error| ToolError::Execution(format!("OpenAI OAuth unavailable: {error}")))?;
    let encoded = crate::http_bridge::run_http(async {
        client
            .generate_image(
                &model,
                prompt,
                input.size.map(ImageSize::as_str),
                input.quality.map(ImageQuality::as_str),
            )
            .await
            .map_err(|error| ToolError::Execution(format!("image generation failed: {error}")))
    })?;

    let saved = persist_generated_image(context, &encoded)?;
    to_pretty_json(json!({
        "status": "generated",
        "path": saved.relative_path,
        "media_type": saved.media_type,
        "bytes": saved.bytes,
        "attached_to_result": saved.attached_to_result,
        "model": model,
    }))
}

#[derive(Debug)]
struct SavedImage {
    relative_path: String,
    media_type: String,
    bytes: usize,
    attached_to_result: bool,
}

fn persist_generated_image(
    context: &ToolContext,
    encoded: &str,
) -> Result<SavedImage, ToolError> {
    if encoded.len() > MAX_GENERATED_IMAGE_BASE64_BYTES {
        return Err(ToolError::Execution(format!(
            "generated image payload exceeded the {MAX_GENERATED_IMAGE_BASE64_BYTES} byte base64 limit"
        )));
    }
    let bytes = STANDARD.decode(encoded).map_err(|error| {
        ToolError::Execution(format!("provider returned invalid image base64: {error}"))
    })?;
    let media_type = sniff_image_mime(&bytes).ok_or_else(|| {
        ToolError::Execution("provider returned an unsupported image format".to_string())
    })?;
    let extension = match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => {
            return Err(ToolError::Execution(format!(
                "provider returned unsupported media type `{media_type}`"
            )));
        }
    };

    let root = workspace_root(context)?;
    let output_dir = checked_generated_image_dir(&root)?;
    let output_path = unique_output_path(&output_dir, extension);
    write_new_file(&output_path, &bytes)?;

    let attached_to_result = stage_for_model(context, media_type, &bytes);
    let relative_path = output_path
        .strip_prefix(&root)
        .unwrap_or(&output_path)
        .to_string_lossy()
        .into_owned();
    Ok(SavedImage {
        relative_path,
        media_type: media_type.to_string(),
        bytes: bytes.len(),
        attached_to_result,
    })
}

fn workspace_root(context: &ToolContext) -> Result<PathBuf, ToolError> {
    let root = context
        .workspace_root
        .as_deref()
        .or(context.cwd.as_deref())
        .map(Path::to_path_buf)
        .map_or_else(std::env::current_dir, Ok)
        .map_err(|error| ToolError::Execution(format!("cannot resolve workspace: {error}")))?;
    fs::canonicalize(&root).map_err(|error| {
        ToolError::Execution(format!(
            "cannot canonicalize workspace `{}`: {error}",
            root.display()
        ))
    })
}

fn checked_generated_image_dir(root: &Path) -> Result<PathBuf, ToolError> {
    let zo_dir = checked_child_dir(root, &root.join(".zo"))?;
    checked_child_dir(root, &zo_dir.join("generated-images"))
}

fn checked_child_dir(root: &Path, path: &Path) -> Result<PathBuf, ToolError> {
    if path.exists() {
        let canonical = fs::canonicalize(path).map_err(|error| {
            ToolError::Execution(format!("cannot resolve `{}`: {error}", path.display()))
        })?;
        if !canonical.starts_with(root) {
            return Err(ToolError::PermissionDenied {
                tool: "imagegen".to_string(),
                reason: format!(
                    "generated image directory escapes workspace: {}",
                    canonical.display()
                ),
            });
        }
        if !canonical.is_dir() {
            return Err(ToolError::Execution(format!(
                "generated image path is not a directory: {}",
                canonical.display()
            )));
        }
        return Ok(canonical);
    }

    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(ToolError::Execution(format!(
                "cannot create `{}`: {error}",
                path.display()
            )));
        }
    }
    let canonical = fs::canonicalize(path).map_err(|error| {
        ToolError::Execution(format!("cannot resolve `{}`: {error}", path.display()))
    })?;
    if !canonical.starts_with(root) {
        return Err(ToolError::PermissionDenied {
            tool: "imagegen".to_string(),
            reason: format!(
                "generated image directory escapes workspace: {}",
                canonical.display()
            ),
        });
    }
    Ok(canonical)
}

fn unique_output_path(output_dir: &Path, extension: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let sequence = IMAGE_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    output_dir.join(format!(
        "image-{timestamp}-{}-{sequence}.{extension}",
        std::process::id()
    ))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            ToolError::Execution(format!("cannot create `{}`: {error}", path.display()))
        })?;
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(ToolError::Execution(format!(
            "cannot write `{}`: {error}",
            path.display()
        )));
    }
    Ok(())
}

fn stage_for_model(context: &ToolContext, media_type: &str, bytes: &[u8]) -> bool {
    let staged = match guard_image_bytes(bytes) {
        ImageGuardOutcome::Keep => Some((media_type.to_string(), STANDARD.encode(bytes))),
        ImageGuardOutcome::Rescaled { media_type, bytes } => {
            Some((media_type, STANDARD.encode(bytes)))
        }
        ImageGuardOutcome::DropOversized { .. } => None,
    };
    if let Some(image) = staged {
        context
            .image_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(image);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn model_gate_accepts_current_gpt_and_rejects_other_providers() {
        assert!(supports_model("gpt-5.6-sol"));
        assert!(supports_model("gpt-5.6-terra"));
        assert!(!supports_model("gpt-5.5"));
        assert!(!supports_model("claude-opus-5"));
        assert!(!supports_model("gemini-3.6-flash"));
    }

    #[test]
    fn generated_image_is_saved_and_staged() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("zo-imagegen-{unique}"));
        fs::create_dir(&root).expect("create fixture root");
        let mut context = ToolContext::new();
        context.workspace_root = Some(root.clone());
        context.cwd = Some(root.clone());

        let saved = persist_generated_image(&context, &STANDARD.encode(PNG_1X1))
            .expect("persist generated image");
        assert_eq!(saved.media_type, "image/png");
        assert_eq!(saved.bytes, PNG_1X1.len());
        assert!(saved.attached_to_result);
        assert!(saved.relative_path.starts_with(".zo/generated-images/image-"));
        assert!(root.join(&saved.relative_path).is_file());
        let staged = context
            .image_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].0, "image/png");

        drop(staged);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn generated_image_directory_cannot_escape_through_a_symlink() {
        use std::os::unix::fs::symlink;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("zo-imagegen-link-{unique}"));
        let root = base.join("workspace");
        let outside = base.join("outside");
        fs::create_dir_all(&root).expect("create workspace");
        fs::create_dir_all(&outside).expect("create outside directory");
        symlink(&outside, root.join(".zo")).expect("create escape symlink");
        let mut context = ToolContext::new();
        context.workspace_root = Some(root);

        let error = persist_generated_image(&context, &STANDARD.encode(PNG_1X1))
            .expect_err("symlink escape must be rejected");
        assert!(matches!(error, ToolError::PermissionDenied { .. }));

        let _ = fs::remove_dir_all(base);
    }
}
