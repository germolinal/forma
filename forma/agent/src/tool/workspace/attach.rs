//! Turning a workspace file into something a model can look at.
//!
//! An attachment lives in the conversation as a reference — a
//! [`FormaWorkspaceFilePart`], which names a workspace and a path — and is
//! turned into bytes only when a request is being built. That keeps a stored
//! session small (a PDF as base64 would not fit one database document) and
//! means a later turn sees the file as it is now.
//!
//! Every provider is fed the same way: the bytes inline, base64 in a data URL.
//! Google's `gs://` references would save the encoding, but only there, and
//! this has to run wherever Forma is deployed.

use base64::Engine;
use image::ImageFormat;
use protocols::{FormaDB, FormaError};
use protocols::{
    FormaFile, FormaFilePart, FormaImagePart, FormaImageUrl, FormaMessagePart,
    FormaWorkspaceFilePart,
};
use tracing::warn;

use crate::{CallContext, Tool, tool::workspace::store_for_workspace};

/// The largest attachment that may go to a model, after any shrinking.
///
/// Uploads are capped at 10 MiB by the application, but an agent writes to a
/// workspace without going through it, and files can arrive in the bucket by
/// other means, so the cap is applied again here — on what is actually sent.
pub(crate) const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

/// The longest edge a picture keeps when it is sent.
///
/// Models downsample anything larger, so the pixels above this are paid for
/// twice — in bandwidth and in tokens — and seen by nobody. It applies to
/// every turn that follows, too, since an attachment stays in the
/// conversation.
const MAX_IMAGE_EDGE: u32 = 1536;

/// Refuses a document that could never be sent, so the tool call that asked
/// for it can say so instead of the failure landing on the next request.
///
/// Only the plainly impossible is refused here: a picture is not shrunk yet
/// (that happens when the request is built), so a large one passes and is
/// judged again, smaller, on the way out.
pub fn check_attachable(doc: &protocols::AttachedDoc) -> Result<(), FormaError> {
    if doc.bytes.is_empty() {
        return Err(FormaError::Input(format!("'{}' is empty", doc.path)));
    }
    if !is_image(&doc.media_type) && doc.bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(FormaError::Input(format!(
            "'{}' is {} and cannot be attached; the limit is {} MiB",
            doc.path,
            human_size(doc.bytes.len()),
            MAX_ATTACHMENT_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

/// Resolves one reference into a part carrying the file itself.
///
/// The workspace is reached through the node's own tools, so an agent only
/// ever gets a file it still has a granted way to read: a reference in an old
/// message cannot outlive the access it was made under.
pub async fn resolve_workspace_file<DB: FormaDB>(
    part: &FormaWorkspaceFilePart,
    tools: &[Tool],
    ctx: &CallContext<DB>,
) -> Result<FormaMessagePart, FormaError> {
    let store = store_for_workspace(tools, &part.workspace_id, ctx).await?;
    let doc = store.attach(&part.path).await?;

    let filename = doc.path.rsplit('/').next().unwrap_or(&doc.path).to_string();
    let (bytes, media_type) = if is_image(&doc.media_type) {
        // Decoding and re-encoding a photo takes tens of milliseconds of CPU:
        // done in place, it would hold up every other run sharing the thread.
        let media_type = doc.media_type;
        tokio::task::spawn_blocking(move || shrink_image(doc.bytes, &media_type))
            .await
            .map_err(|e| FormaError::Execution(format!("could not shrink a picture: {e}")))?
    } else {
        (doc.bytes, doc.media_type)
    };

    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(FormaError::Input(format!(
            "'{}' is {} and cannot be attached; the limit is {} MiB",
            doc.path,
            human_size(bytes.len()),
            MAX_ATTACHMENT_BYTES / (1024 * 1024)
        )));
    }

    let url = data_url(&media_type, &bytes);
    Ok(if is_image(&media_type) {
        FormaMessagePart::ImageUrl(FormaImagePart {
            image_url: FormaImageUrl { url, detail: None },
        })
    } else {
        FormaMessagePart::File(FormaFilePart {
            file: FormaFile {
                file_data: Some(url),
                file_id: None,
                filename: Some(filename),
            },
        })
    })
}

/// Whether a document of this type goes to the model as a file: a picture
/// every provider takes, or a PDF. Anything else is either text or nothing a
/// model can be given.
pub fn is_attachable(media_type: &str) -> bool {
    image_format(media_type).is_some()
        || media_type.split(';').next().unwrap_or("").trim() == "application/pdf"
}

fn is_image(media_type: &str) -> bool {
    media_type.starts_with("image/")
}

/// `data:<type>;base64,<payload>`, the one shape every provider adapter here
/// understands.
fn data_url(media_type: &str, bytes: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        media_type,
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// Shrinks a picture to [`MAX_IMAGE_EDGE`] on its longest side, keeping its
/// format.
///
/// Best-effort by design: a picture this cannot decode or re-encode is sent as
/// it is, and the size cap still applies to it. Refusing to attach something a
/// model would have read perfectly well is the worse failure.
fn shrink_image(bytes: Vec<u8>, media_type: &str) -> (Vec<u8>, String) {
    let Some(format) = image_format(media_type) else {
        return (bytes, media_type.to_string());
    };
    let decoded = match image::load_from_memory_with_format(&bytes, format) {
        Ok(img) => img,
        Err(e) => {
            warn!("could not read an attached picture, sending it as it is: {e}");
            return (bytes, media_type.to_string());
        }
    };
    if decoded.width() <= MAX_IMAGE_EDGE && decoded.height() <= MAX_IMAGE_EDGE {
        return (bytes, media_type.to_string());
    }

    let resized = decoded.thumbnail(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE);
    let mut out = std::io::Cursor::new(Vec::new());
    // An animation loses its frames on the way through, so a GIF is written as
    // a PNG: one still picture, honestly typed, rather than a broken GIF.
    let (out_format, out_type) = match format {
        ImageFormat::Gif => (ImageFormat::Png, "image/png"),
        other => (other, media_type),
    };
    match resized.write_to(&mut out, out_format) {
        Ok(()) => (out.into_inner(), out_type.to_string()),
        Err(e) => {
            warn!("could not shrink an attached picture, sending it as it is: {e}");
            (bytes, media_type.to_string())
        }
    }
}

/// The decoder for a media type, or `None` for a picture this cannot handle.
fn image_format(media_type: &str) -> Option<ImageFormat> {
    match media_type.split(';').next()?.trim() {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

fn human_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn png_of(width: u32, height: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::new(width, height));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn test_data_url_is_what_the_adapters_expect() {
        assert_eq!(
            data_url("application/pdf", b"hi"),
            "data:application/pdf;base64,aGk="
        );
    }

    /// A picture bigger than a model will look at is sent smaller; the format
    /// it was stored in is kept.
    #[test]
    fn test_shrink_image_bounds_the_longest_edge() {
        let (bytes, media_type) = shrink_image(png_of(4000, 1000), "image/png");
        assert_eq!(media_type, "image/png");
        let out = image::load_from_memory(&bytes).expect("still a picture");
        assert_eq!(out.width(), MAX_IMAGE_EDGE);
        assert_eq!(out.height(), MAX_IMAGE_EDGE / 4);
    }

    /// One that is already small enough is passed through untouched, rather
    /// than re-encoded into something slightly different.
    #[test]
    fn test_shrink_image_leaves_small_pictures_alone() {
        let original = png_of(64, 64);
        let (bytes, media_type) = shrink_image(original.clone(), "image/png");
        assert_eq!(bytes, original);
        assert_eq!(media_type, "image/png");
    }

    /// Anything that is not a picture this can read goes through as it is: the
    /// size cap, not the decoder, is what refuses a file.
    #[test]
    fn test_shrink_image_passes_through_what_it_cannot_read() {
        let (bytes, media_type) = shrink_image(b"%PDF-1.7".to_vec(), "application/pdf");
        assert_eq!(bytes, b"%PDF-1.7");
        assert_eq!(media_type, "application/pdf");

        let broken = shrink_image(b"not a png".to_vec(), "image/png");
        assert_eq!(broken.0, b"not a png");
    }

    #[test]
    fn test_image_format_reads_a_parameterised_type() {
        assert_eq!(image_format("image/png"), Some(ImageFormat::Png));
        assert_eq!(
            image_format("image/jpeg; charset=binary"),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(image_format("application/pdf"), None);
    }

    #[test]
    fn test_only_pictures_and_pdfs_are_attached() {
        assert!(is_attachable("image/png"));
        assert!(is_attachable("application/pdf; charset=binary"));
        // A picture written as text is read, not looked at.
        assert!(!is_attachable("image/svg+xml"));
        assert!(!is_attachable("text/markdown"));
        assert!(!is_attachable("application/octet-stream"));
    }
}
